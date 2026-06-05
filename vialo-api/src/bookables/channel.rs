use crate::events::StatusChannel;
use crate::http::bookables::{
    models::BookableAssetStatus,
    permissions::{BookablePerm, require_asset_type_perm},
};
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use uuid::Uuid;

pub struct BookableChannel {
    inner: StatusChannel<i32, BookableAssetStatus>,
    db: Pool<Postgres>,
}

impl BookableChannel {
    pub fn new(name: String, db: Pool<Postgres>) -> Self {
        BookableChannel {
            inner: StatusChannel::new(name),
            db,
        }
    }

    pub async fn subscribe(
        &self,
        asset_type_id: i32,
        user_id: Option<Uuid>,
        slot: Arc<Mutex<HashMap<i32, Arc<String>>>>,
        notify: Arc<Notify>,
    ) {
        if require_asset_type_perm(user_id, asset_type_id, BookablePerm::View, &self.db)
            .await
            .is_ok()
        {
            self.inner.subscribe(asset_type_id, slot, notify).await;
        }
    }

    pub async fn subscribe_notify(&self) -> Arc<Notify> {
        self.inner.subscribe_notify().await
    }

    pub async fn broadcast(&self, routing_key: i32, coalesce_key: i32, value: BookableAssetStatus) {
        self.inner.broadcast(routing_key, coalesce_key, value).await;
    }
}
