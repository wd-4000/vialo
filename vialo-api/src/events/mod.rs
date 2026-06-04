use futures::future::join_all;
use serde::Serialize;
use std::fmt::Debug;
use std::{collections::HashMap, hash::Hash, sync::Arc};
use tokio::sync::{RwLock, mpsc::Sender};

#[derive(Serialize)]
pub struct EventEnvelope<K, V> {
    pub __vlo_channel: String,
    pub __vlo_id: K,
    #[serde(flatten)]
    pub data: V,
}

struct Subscribers<K, V> {
    typed: Vec<Sender<Arc<EventEnvelope<K, V>>>>,
    string: Vec<Sender<Arc<String>>>,
}

impl<K, V> Default for Subscribers<K, V> {
    fn default() -> Self {
        Subscribers {
            typed: Vec::new(),
            string: Vec::new(),
        }
    }
}

pub struct EventChannel<K, V> {
    pub name: String,
    subscribers: Arc<RwLock<HashMap<K, Subscribers<K, V>>>>,
    wildcard_subscribers: Arc<RwLock<Subscribers<K, V>>>,
}

impl<K: Hash + std::cmp::Eq + Send + std::marker::Sync + Debug + Serialize + Clone, V: Serialize>
    EventChannel<K, V>
{
    pub fn new(name: String) -> Self {
        EventChannel {
            name,
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            wildcard_subscribers: Arc::new(RwLock::new(Subscribers::default())),
        }
    }
    pub async fn subscribe_wildcard(&self, sender: Sender<Arc<EventEnvelope<K, V>>>) {
        self.wildcard_subscribers.write().await.typed.push(sender);
    }
    pub async fn subscribe(&self, key: K, sender: Sender<Arc<EventEnvelope<K, V>>>) {
        self.subscribers
            .write()
            .await
            .entry(key)
            .or_default()
            .typed
            .push(sender);
    }
    pub async fn subscribe_string(&self, key: K, sender: Sender<Arc<String>>) {
        self.subscribers
            .write()
            .await
            .entry(key)
            .or_default()
            .string
            .push(sender);
    }
    pub async fn subscribe_wildcard_string(&self, sender: Sender<Arc<String>>) {
        self.wildcard_subscribers.write().await.string.push(sender);
    }
    pub async fn broadcast(&self, key: K, value: V) {
        let (string_senders, typed_senders) = {
            let subs = self.subscribers.read().await;
            let wildcard_subs = self.wildcard_subscribers.read().await;

            let string_senders: Vec<_> = wildcard_subs
                .string
                .iter()
                .chain(subs.get(&key).map(|s| s.string.iter()).into_iter().flatten())
                .cloned()
                .collect();
            let typed_senders: Vec<_> = wildcard_subs
                .typed
                .iter()
                .chain(subs.get(&key).map(|s| s.typed.iter()).into_iter().flatten())
                .cloned()
                .collect();

            (string_senders, typed_senders)
        };

        if string_senders.is_empty() && typed_senders.is_empty() {
            return;
        }

        let envelope = EventEnvelope {
            __vlo_channel: self.name.clone(),
            __vlo_id: key.clone(),
            data: value,
        };

        let json_string = if !string_senders.is_empty() {
            Some(Arc::new(
                serde_json::to_string(&envelope).unwrap_or_default(),
            ))
        } else {
            None
        };

        let mut any_dead = false;

        if !typed_senders.is_empty() {
            let envelope = Arc::new(envelope);
            let results = join_all(
                typed_senders
                    .iter()
                    .map(|sender| sender.send(Arc::clone(&envelope))),
            )
            .await;
            any_dead |= results.iter().any(|r| r.is_err());
        }

        if let Some(json_string) = json_string {
            let results = join_all(
                string_senders
                    .iter()
                    .map(|sender| sender.send(Arc::clone(&json_string))),
            )
            .await;
            any_dead |= results.iter().any(|r| r.is_err());
        }

        if any_dead {
            {
                let mut subs = self.subscribers.write().await;
                if let Some(entry) = subs.get_mut(&key) {
                    entry.string.retain(|tx| !tx.is_closed());
                    entry.typed.retain(|tx| !tx.is_closed());
                    if entry.string.is_empty() && entry.typed.is_empty() {
                        subs.remove(&key);
                    }
                }
            }
            {
                let mut wildcard = self.wildcard_subscribers.write().await;
                wildcard.string.retain(|tx| !tx.is_closed());
                wildcard.typed.retain(|tx| !tx.is_closed());
            }
        }
    }
}
