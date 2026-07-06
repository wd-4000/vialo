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

type AuthCallback<K> =
    Arc<dyn Fn(K, Vec<Option<Uuid>>) -> BoxFuture<'static, HashSet<Option<Uuid>>> + Send + Sync>;

struct Subscriber<K> {
    account_id: Option<Uuid>,
    slot: Weak<Mutex<HashMap<K, Arc<String>>>>,
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
    wildcard_notifiers: tokio::sync::RwLock<Vec<Weak<Notify>>>,
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
            wildcard_notifiers: tokio::sync::RwLock::new(Vec::new()),
            auth_filter: OnceLock::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Install an authorization filter. Must be called before any broadcast.
    /// `callback` receives (routing_key, subscriber account_ids) and returns
    /// the set of account_ids that are still authorized.
    pub fn set_auth_filter(&self, callback: AuthCallback<K>) {
        assert!(
            self.auth_filter.set(callback).is_ok(),
            "set_auth_filter called more than once"
        );
    }

    pub async fn subscribe(
        &self,
        key: K,
        account_id: Option<Uuid>,
        slot: Arc<Mutex<HashMap<K, Arc<String>>>>,
        notify: Arc<Notify>,
    ) {
        self.subscribers
            .write()
            .await
            .entry(key)
            .or_default()
            .push(Subscriber {
                account_id,
                slot: Arc::downgrade(&slot),
                notify: Arc::downgrade(&notify),
            });
    }

    pub async fn subscribe_notify(&self) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        self.wildcard_notifiers
            .write()
            .await
            .push(Arc::downgrade(&notify));
        notify
    }

    pub async fn broadcast(&self, routing_key: K, coalesce_key: K, value: V) {
        let envelope = EventEnvelope {
            __vlo_channel: self.name.clone(),
            __vlo_id: routing_key.clone(),
            data: value,
        };
        let json = Arc::new(serde_json::to_string(&envelope).unwrap_or_default());

        // collect account_ids under read lock, then run filter outside lock
        let authorized = {
            let subs = self.subscribers.read().await;
            let Some(list) = subs.get(&routing_key) else {
                // Still notify wildcard listeners even with no keyed subscribers
                self.notify_wildcard().await;
                return;
            };

            if let Some(filter) = self.auth_filter.get() {
                let account_ids: Vec<Option<Uuid>> = list.iter().map(|s| s.account_id).collect();
                // drop read lock before running the async filter
                drop(subs);
                Some(filter(routing_key.clone(), account_ids).await)
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
                    list.retain(|sub| authorized.contains(&sub.account_id));
                }

                list.retain(|sub| match (sub.slot.upgrade(), sub.notify.upgrade()) {
                    (Some(slot), Some(notify)) => {
                        slot.lock()
                            .unwrap()
                            .insert(coalesce_key.clone(), Arc::clone(&json));
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

        self.notify_wildcard().await;
    }

    async fn notify_wildcard(&self) {
        let mut notifiers = self.wildcard_notifiers.write().await;
        notifiers.retain(|weak| match weak.upgrade() {
            Some(notify) => {
                notify.notify_one();
                true
            }
            None => false,
        });
    }
}
