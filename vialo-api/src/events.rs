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
    /// `None` subscribes to every routing key
    subscribers: tokio::sync::RwLock<HashMap<Option<K>, Vec<Subscriber<K>>>>,
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

    pub async fn subscribe(
        &self,
        key: Option<K>,
        auth: Auth,
        slot: SlotMap<K>,
        notify: Arc<Notify>,
    ) {
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

    /// Deliver `items` as (coalesce_key, value) pairs under one routing key
    pub async fn broadcast(&self, routing_key: K, items: Vec<(K, V)>) {
        let keys = [Some(routing_key.clone()), None];

        // collect subscriber auths under read lock, then run filter outside lock
        let auths: HashSet<Auth> = {
            let subs = self.subscribers.read().await;
            keys.iter()
                .filter_map(|k| subs.get(k))
                .flatten()
                .map(|s| s.auth.clone())
                .collect()
        };
        if auths.is_empty() || items.is_empty() {
            return;
        }

        let authorized = match self.auth_filter.get() {
            Some(filter) => Some(filter(routing_key.clone(), auths.into_iter().collect()).await),
            None => None,
        };

        let messages: Vec<((String, K), Arc<String>)> = items
            .into_iter()
            .map(|(coalesce_key, value)| {
                let envelope = EventEnvelope {
                    __vlo_channel: self.name.clone(),
                    __vlo_id: routing_key.clone(),
                    data: value,
                };
                let json = serde_json::to_string(&envelope).unwrap_or_default();
                ((self.name.clone(), coalesce_key), Arc::new(json))
            })
            .collect();

        // unauthorized subscribers are skipped, not pruned: the filter is rerun per
        // broadcast, so regaining access resumes delivery
        let mut subs = self.subscribers.write().await;
        for key in keys {
            let Some(list) = subs.get_mut(&key) else {
                continue;
            };

            list.retain(|sub| {
                let (Some(slot), Some(notify)) = (sub.slot.upgrade(), sub.notify.upgrade()) else {
                    return false;
                };
                if authorized.as_ref().is_none_or(|a| a.contains(&sub.auth)) {
                    let mut slot = slot.lock().unwrap();
                    for (k, json) in &messages {
                        slot.insert(k.clone(), Arc::clone(json));
                    }
                    drop(slot);
                    notify.notify_one();
                }
                true
            });

            if list.is_empty() {
                subs.remove(&key);
            }
        }
    }
}
