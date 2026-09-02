use crate::config::KioskConfig;
use crate::events::{Auth, StatusChannel};
use crate::http::bookables::models::{BookableAssetQueue, BookableAssetStatus};
use crate::http::bookables::permissions::BookablePerm;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Notify;
use uuid::Uuid;

pub struct BookableChannel {
    inner: StatusChannel<i32, BookableAssetStatus>,
    db: Pool<Postgres>,
    kiosk: Option<Arc<KioskConfig>>,
}

/// Permission recheck on every event
async fn auth_filter(
    db: Pool<Postgres>,
    kiosk: Option<Arc<KioskConfig>>,
    min_perm: BookablePerm,
    allow_public: bool,
    asset_type_id: i32,
    auths: Vec<Auth>,
) -> HashSet<Auth> {
    let mut named: Vec<Uuid> = Vec::new();
    let mut has_anonymous = false;
    let mut has_kiosk = false;
    let mut check_public = false;
    let mut authorized: HashSet<Auth> = HashSet::new();

    for auth in &auths {
        match auth {
            Auth::Account(id) => named.push(*id),
            Auth::Anonymous => {
                has_anonymous = true;
                check_public = true;
            }
            Auth::Kiosk => {
                has_kiosk = true;
                if kiosk.as_ref().is_some_and(|k| k.allows(asset_type_id)) {
                    authorized.insert(Auth::Kiosk);
                } else {
                    check_public = true;
                }
            }
        }
    }

    if !named.is_empty() {
        let rows = sqlx::query_scalar!(
            r#"
            SELECT id AS "id!" FROM unnest($2::uuid[]) AS t(id)
                WHERE account_bookable_perm_exists(id, $1, $3)
            "#,
            asset_type_id,
            &named,
            min_perm.clone() as _,
        )
        .fetch_all(&db)
        .await
        .unwrap_or_default();

        authorized.extend(rows.into_iter().map(Auth::Account));
    }

    // Public access governs both anonymous subscribers and kiosks
    // without their own grant on this channel
    if allow_public && check_public {
        let public_ok: bool = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                SELECT 1 FROM bookable_asset_type_group_perms
                WHERE asset_type_id = $1 AND group_id IS NULL AND perm >= $2
            ) AS "allowed: bool""#,
            asset_type_id,
            min_perm as _,
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
}

/// Subscribe-time permission check
async fn auth_allows(
    db: &Pool<Postgres>,
    kiosk: Option<Arc<KioskConfig>>,
    min_perm: BookablePerm,
    allow_public: bool,
    asset_type_id: i32,
    auth: &Auth,
) -> bool {
    auth_filter(
        db.clone(),
        kiosk,
        min_perm,
        allow_public,
        asset_type_id,
        vec![auth.clone()],
    )
    .await
    .contains(auth)
}

impl BookableChannel {
    pub fn new(name: String, db: Pool<Postgres>, kiosk: Option<Arc<KioskConfig>>) -> Self {
        let inner = StatusChannel::new(name);

        let filter_db = db.clone();
        let filter_kiosk = kiosk.clone();
        inner.set_auth_filter(Arc::new(move |asset_type_id: i32, auths: Vec<Auth>| {
            Box::pin(auth_filter(
                filter_db.clone(),
                filter_kiosk.clone(),
                BookablePerm::View,
                true,
                asset_type_id,
                auths,
            ))
        }));

        BookableChannel { inner, db, kiosk }
    }

    pub async fn subscribe(
        &self,
        asset_type_id: i32,
        auth: Auth,
        slot: crate::events::SlotMap<i32>,
        notify: Arc<Notify>,
    ) {
        if auth_allows(
            &self.db,
            self.kiosk.clone(),
            BookablePerm::View,
            true,
            asset_type_id,
            &auth,
        )
        .await
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
    kiosk: Option<Arc<KioskConfig>>,
}

impl BookableQueueChannel {
    pub fn new(name: String, db: Pool<Postgres>, kiosk: Option<Arc<KioskConfig>>) -> Self {
        let inner = StatusChannel::new(name);

        let filter_db = db.clone();
        let filter_kiosk = kiosk.clone();
        inner.set_auth_filter(Arc::new(move |asset_type_id: i32, auths: Vec<Auth>| {
            Box::pin(auth_filter(
                filter_db.clone(),
                filter_kiosk.clone(),
                BookablePerm::Book,
                false,
                asset_type_id,
                auths,
            ))
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
        if auth_allows(
            &self.db,
            self.kiosk.clone(),
            BookablePerm::Book,
            false,
            asset_type_id,
            &auth,
        )
        .await
        {
            self.inner
                .subscribe(asset_type_id, auth, slot, notify)
                .await;
        }
    }

    pub async fn broadcast(&self, routing_key: i32, coalesce_key: i32, value: BookableAssetQueue) {
        self.inner.broadcast(routing_key, coalesce_key, value).await;
    }
}
