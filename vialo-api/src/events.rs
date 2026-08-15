use futures::future::BoxFuture;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Serialize)]
pub struct EventEnvelope<K, V> {
    pub __vlo_channel: String,
    pub __vlo_id: K,
    #[serde(flatten)]
    pub data: V,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Auth {
    Anonymous,
    Account(Uuid),
    Kiosk,
}

type AuthCallback<K> = Arc<dyn Fn(K, Vec<Auth>) -> BoxFuture<'static, HashSet<Auth>> + Send + Sync>;

pub type SlotMap<K> = Arc<Mutex<HashMap<(String, K), Arc<String>>>>;

struct Subscriber<K> {
    auth: Auth,
    slot: Weak<Mutex<HashMap<(String, K), Arc<String>>>>,
    notify: Weak<Notify>,
}

/// broadcast channel that delivers the latest value per coalescing key to each subscriber.
///
/// example for bookables: subscribe via `routing_key` (asset type) and receive updates for any
/// value broadcast under that key. `coalesce_key` (asset id), used to make sure that
/// in the event of backpressure the most recent value for each asset is delivered
pub struct StatusChannel<K, V> {
    name: String,
    subscribers: tokio::sync::RwLock<HashMap<K, Vec<Subscriber<K>>>>,
    auth_filter: OnceLock<AuthCallback<K>>,
    _phantom: std::marker::PhantomData<V>,
}

impl<K: Hash + Eq + Clone + Debug + Serialize + Send + Sync + 'static, V: Serialize>
    StatusChannel<K, V>
{
    pub fn new(name: String) -> Self {
        StatusChannel {
            name,
            subscribers: tokio::sync::RwLock::new(HashMap::new()),
            auth_filter: OnceLock::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Install an authorization filter. Must be called before any broadcast.
    /// `callback` receives (routing_key, subscriber auths) and returns the set
    /// of auths that are still authorized.
    pub fn set_auth_filter(&self, callback: AuthCallback<K>) {
        assert!(
            self.auth_filter.set(callback).is_ok(),
            "set_auth_filter called more than once"
        );
    }

    pub async fn subscribe(&self, key: K, auth: Auth, slot: SlotMap<K>, notify: Arc<Notify>) {
        self.subscribers
            .write()
            .await
            .entry(key)
            .or_default()
            .push(Subscriber {
                auth,
                slot: Arc::downgrade(&slot),
                notify: Arc::downgrade(&notify),
            });
    }

    pub async fn broadcast(&self, routing_key: K, coalesce_key: K, value: V) {
        let envelope = EventEnvelope {
            __vlo_channel: self.name.clone(),
            __vlo_id: routing_key.clone(),
            data: value,
        };
        let json = Arc::new(serde_json::to_string(&envelope).unwrap_or_default());

        // collect subscriber auths under read lock, then run filter outside lock
        let authorized = {
            let subs = self.subscribers.read().await;
            let Some(list) = subs.get(&routing_key) else {
                return;
            };

            if let Some(filter) = self.auth_filter.get() {
                let auths: Vec<Auth> = list.iter().map(|s| s.auth.clone()).collect();
                // drop read lock before running the async filter
                drop(subs);
                Some(filter(routing_key.clone(), auths).await)
            } else {
                drop(subs);
                None
            }
        };

        // prune unauthorized, deliver to remaining
        {
            let mut subs = self.subscribers.write().await;
            if let Some(list) = subs.get_mut(&routing_key) {
                if let Some(ref authorized) = authorized {
                    list.retain(|sub| authorized.contains(&sub.auth));
                }

                list.retain(|sub| match (sub.slot.upgrade(), sub.notify.upgrade()) {
                    (Some(slot), Some(notify)) => {
                        slot.lock()
                            .unwrap()
                            .insert((self.name.clone(), coalesce_key.clone()), Arc::clone(&json));
                        notify.notify_one();
                        true
                    }
                    _ => false,
                });

                if list.is_empty() {
                    subs.remove(&routing_key);
                }
            }
        }
    }
}
