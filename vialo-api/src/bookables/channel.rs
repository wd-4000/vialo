use crate::config::KioskConfig;
use crate::events::{Auth, StatusChannel};
use crate::http::bookables::models::{BookableAssetQueue, BookableAssetStatus};
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
        inner.set_auth_filter(Arc::new(move |asset_type_id: i32, auths: Vec<Auth>| {
            let db = filter_db.clone();
            Box::pin(async move {
                let mut named: Vec<Uuid> = Vec::new();
                let mut has_anonymous = false;
                let mut has_kiosk = false;
                for auth in &auths {
                    match auth {
                        Auth::Account(id) => named.push(*id),
                        Auth::Anonymous => has_anonymous = true,
                        Auth::Kiosk => has_kiosk = true,
                    }
                }
                let mut authorized: HashSet<Auth> = HashSet::new();

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

                    authorized.extend(rows.into_iter().map(Auth::Account));
                }

                // Public access governs both anonymous subscribers and kiosks
                // without their own grant on this channel
                if has_anonymous || has_kiosk {
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
                        if has_anonymous {
                            authorized.insert(Auth::Anonymous);
                        }
                        if has_kiosk {
                            authorized.insert(Auth::Kiosk);
                        }
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
        auth: Auth,
        slot: crate::events::SlotMap<i32>,
        notify: Arc<Notify>,
    ) {
        // Fast-fail permission check at subscribe time
        let account_id = match auth {
            Auth::Account(id) => Some(id),
            Auth::Anonymous | Auth::Kiosk => None,
        };
        if crate::http::bookables::permissions::require_asset_type_perm(
            account_id,
            asset_type_id,
            crate::http::bookables::permissions::BookablePerm::View,
            &self.db,
        )
        .await
        .is_ok()
        {
            self.inner
                .subscribe(asset_type_id, auth, slot, notify)
                .await;
        }
    }

    pub async fn broadcast(&self, routing_key: i32, coalesce_key: i32, value: BookableAssetStatus) {
        self.inner.broadcast(routing_key, coalesce_key, value).await;
    }
}

pub struct BookableQueueChannel {
    inner: StatusChannel<i32, BookableAssetQueue>,
    db: Pool<Postgres>,
    kiosk: Option<KioskConfig>,
}

impl BookableQueueChannel {
    pub fn new(name: String, db: Pool<Postgres>, kiosk: Option<KioskConfig>) -> Self {
        let inner = StatusChannel::new(name);

        let filter_db = db.clone();
        let filter_kiosk = kiosk.clone();
        inner.set_auth_filter(Arc::new(move |asset_type_id: i32, auths: Vec<Auth>| {
            let db = filter_db.clone();
            let kiosk = filter_kiosk.clone();
            Box::pin(async move {
                // NOT public
                let mut named: Vec<Uuid> = Vec::new();
                let mut has_kiosk = false;
                for auth in &auths {
                    match auth {
                        Auth::Account(id) => named.push(*id),
                        Auth::Anonymous => {}
                        Auth::Kiosk => has_kiosk = true,
                    }
                }

                let mut authorized: HashSet<Auth> = HashSet::new();

                if !named.is_empty() {
                    let rows = sqlx::query_scalar!(
                        r#"
                        SELECT id FROM accounts_people WHERE id = ANY($2)
                            AND account_bookable_perm_exists(id, $1, 'book')
                        "#,
                        asset_type_id,
                        &named,
                    )
                    .fetch_all(&db)
                    .await
                    .unwrap_or_default();

                    authorized.extend(rows.into_iter().map(Auth::Account));
                }

                if has_kiosk && kiosk.is_some_and(|k| k.allows(asset_type_id)) {
                    authorized.insert(Auth::Kiosk);
                }

                authorized
            })
        }));

        BookableQueueChannel { inner, db, kiosk }
    }

    pub async fn subscribe(
        &self,
        asset_type_id: i32,
        auth: Auth,
        slot: crate::events::SlotMap<i32>,
        notify: Arc<Notify>,
    ) {
        match auth {
            Auth::Anonymous => return,
            Auth::Kiosk => {
                if !self.kiosk.as_ref().is_some_and(|k| k.allows(asset_type_id)) {
                    return;
                }
            }
            Auth::Account(account_id) => {
                if crate::http::bookables::permissions::require_asset_type_perm(
                    Some(account_id),
                    asset_type_id,
                    crate::http::bookables::permissions::BookablePerm::Book,
                    &self.db,
                )
                .await
                .is_err()
                {
                    return;
                }
            }
        }

        self.inner
            .subscribe(asset_type_id, auth, slot, notify)
            .await;
    }

    pub async fn broadcast(&self, routing_key: i32, coalesce_key: i32, value: BookableAssetQueue) {
        self.inner.broadcast(routing_key, coalesce_key, value).await;
    }
}
