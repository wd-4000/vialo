use sqlx::PgExecutor;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;
use crate::http::util::{User, VialoError};

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type)]
#[sqlx(type_name = "bookable_perm", rename_all = "snake_case")]
pub enum BookablePerm {
    View,
    Book,
    Admin,
}

pub async fn require_asset_type_perm<'c, T: PgExecutor<'c>>(
    user_id: Option<Uuid>,
    asset_type_id: i32,
    min_perm: BookablePerm,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT account_bookable_perm_exists($1, $2, $3) AS "allowed: bool""#,
        user_id,
        asset_type_id,
        min_perm as _,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if allowed {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

pub async fn require_asset_type_perm_by_schema<'c, T: PgExecutor<'c>>(
    user_id: Uuid,
    schema_id: i32,
    min_perm: BookablePerm,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT account_bookable_perm_exists($1, (SELECT asset_type_id FROM bookable_schemas WHERE id = $2), $3) AS "allowed: bool""#,
        user_id,
        schema_id,
        min_perm as _,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if allowed {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

pub async fn require_asset_type_perm_by_asset<'c, T: PgExecutor<'c>>(
    user_id: Uuid,
    asset_id: i32,
    min_perm: BookablePerm,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT account_bookable_perm_exists($1, (SELECT asset_type_id FROM bookable_assets WHERE id = $2), $3) AS "allowed: bool""#,
        user_id,
        asset_id,
        min_perm as _,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if allowed {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AssetTypeScope {
    All,
    Only(Vec<i32>),
    None,
}

/// What asset types an account has a particular permission level over
pub async fn resolve_asset_type_scope<'c, T: PgExecutor<'c>>(
    user_id: Uuid,
    min_perm: BookablePerm,
    db: T,
) -> Result<AssetTypeScope, VialoError> {
    let scope = sqlx::query!(
        r#"SELECT
            account_role_exists($1, 'bookable_manager') AS "is_manager!",
            coalesce((
                SELECT array_agg(DISTINCT p.asset_type_id)
                FROM bookable_asset_type_group_perms p
                LEFT JOIN account_group_memberships agm
                    ON agm.group_id = p.group_id AND agm.account_id = $1
                WHERE p.perm >= $2 AND (p.group_id IS NULL OR agm.account_id IS NOT NULL)
            ), '{}'::int[]) AS "asset_types!""#,
        user_id,
        min_perm as _,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?;

    Ok(if scope.is_manager {
        AssetTypeScope::All
    } else if scope.asset_types.is_empty() {
        AssetTypeScope::None
    } else {
        AssetTypeScope::Only(scope.asset_types)
    })
}

/// Who is calling an endpoint that serves kiosk devices as well as accounts
#[derive(Clone)]
pub enum BookableCaller {
    Kiosk(crate::config::KioskConfig),
    Account(Uuid),
    Anonymous,
}

impl axum::extract::FromRequestParts<Arc<AppState>> for BookableCaller {
    type Rejection = VialoError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let presented = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);

        if let Some(kiosk) = presented.and_then(|token| {
            state
                .config
                .bookables
                .as_ref()
                .and_then(|b| b.kiosk.as_ref())
                .filter(|k| k.matches(token))
        }) {
            return Ok(Self::Kiosk(kiosk.clone()));
        }

        Ok(parts
            .extensions
            .get::<Option<User>>()
            .cloned()
            .flatten()
            .map(|user| Self::Account(user.id))
            .unwrap_or(Self::Anonymous))
    }
}
