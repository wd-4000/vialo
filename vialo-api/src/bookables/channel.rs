use crate::events::StatusChannel;
use crate::http::bookables::models::BookableAssetStatus;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Notify;
use uuid::Uuid;

pub struct BookableChannel {
    inner: StatusChannel<i32, BookableAssetStatus>,
    db: Pool<Postgres>,
}

impl BookableChannel {
    pub fn new(name: String, db: Pool<Postgres>) -> Self {
        let inner = StatusChannel::new(name);

        let filter_db = db.clone();
        inner.set_auth_filter(Arc::new(move |asset_type_id: i32, account_ids: Vec<Option<Uuid>>| {
            let db = filter_db.clone();
            Box::pin(async move {
                let named: Vec<Uuid> = account_ids.iter().filter_map(|id| *id).collect();
                let mut authorized: HashSet<Option<Uuid>> = HashSet::new();

                if !named.is_empty() {
                    let rows = sqlx::query_scalar!(
                        r#"
                        SELECT id FROM accounts_people WHERE id = ANY($2)
                            AND account_bookable_perm_exists(id, $1, 'view')
                        "#,
                        asset_type_id,
                        &named,
                    )
                    .fetch_all(&db)
                    .await
                    .unwrap_or_default();

                    authorized.extend(rows.into_iter().map(Some));
                }

                // Check public access for anonymous subscribers
                if account_ids.iter().any(|id| id.is_none()) {
                    let public_ok: bool = sqlx::query_scalar!(
                        r#"SELECT EXISTS (
                            SELECT 1 FROM bookable_asset_type_group_perms
                            WHERE asset_type_id = $1 AND group_id IS NULL AND perm >= 'view'
                        ) AS "allowed: bool""#,
                        asset_type_id,
                    )
                    .fetch_one(&db)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);

                    if public_ok {
                        authorized.insert(None);
                    }
                }

                authorized
            })
        }));

        BookableChannel { inner, db }
    }

    pub async fn subscribe(
        &self,
        asset_type_id: i32,
        account_id: Option<Uuid>,
        slot: Arc<std::sync::Mutex<std::collections::HashMap<i32, Arc<String>>>>,
        notify: Arc<Notify>,
    ) {
        // Fast-fail permission check at subscribe time
        if crate::http::bookables::permissions::require_asset_type_perm(
            account_id,
            asset_type_id,
            crate::http::bookables::permissions::BookablePerm::View,
            &self.db,
        )
        .await
        .is_ok()
        {
            self.inner.subscribe(asset_type_id, account_id, slot, notify).await;
        }
    }

    pub async fn subscribe_notify(&self) -> Arc<Notify> {
        self.inner.subscribe_notify().await
    }

    pub async fn broadcast(
        &self,
        routing_key: i32,
        coalesce_key: i32,
        value: BookableAssetStatus,
    ) {
        self.inner.broadcast(routing_key, coalesce_key, value).await;
    }
}
