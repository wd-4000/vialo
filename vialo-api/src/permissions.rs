use std::sync::Arc;

use axum::{
    Extension,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    http::util::{User, VialoError},
};

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "group_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum GroupRole {
    Member,
    Manager,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema,
)]
#[sqlx(type_name = "app_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AppRole {
    AccountManager,
    CreditManager,
    BookableManager,
    BoardManager,
    HomeManager,
    NetworkManager,
    HistoryViewer,
    HealthViewer,
}

/// Returns `Ok(())` if the user holds the given [`AppRole`] via any of their group memberships,
/// or `Err(`[`VialoError::Forbidden`]`)` otherwise.
pub async fn check_app_role<'c, T: PgExecutor<'c>>(
    user: User,
    role: AppRole,
    db: T,
) -> Result<(), VialoError> {
    let has_role = sqlx::query_scalar!(
        r#"SELECT COUNT(*) > 0 AS "has_role: bool" FROM account_group_memberships agm
         JOIN account_group_app_roles agra ON agm.group_id = agra.group_id
         WHERE agm.account_id = $1 AND agra.role = $2"#,
        user.id,
        role as _,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if has_role {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

/// Returns `Ok(())` if the user is a `manager`-role member of the given group,
/// or `Err(`[`VialoError::Forbidden`]`)` otherwise.
pub async fn check_manager_of_group<'c, T: PgExecutor<'c>>(
    user: User,
    group_id: Uuid,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT COUNT(*) > 0 AS "allowed: bool" FROM account_group_memberships
         WHERE account_id = $1 AND role = 'manager' AND group_id = $2"#,
        user.id,
        group_id
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

/// Returns `Ok(())` if the user is a member (any role) of the given group,
/// or `Err(`[`VialoError::Forbidden`]`)` otherwise.
pub async fn check_member_of_group<'c, T: PgExecutor<'c>>(
    user: User,
    group_id: Uuid,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT COUNT(*) > 0 AS "allowed: bool" FROM account_group_memberships
         WHERE account_id = $1 AND group_id = $2"#,
        user.id,
        group_id
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

/// Returns `Ok(())` if the user is a member (any role) of the given group
/// OR holds the given [`AppRole`]. Useful for routes that can be accessed by
/// either a site-wide staff role or any member of the linked group.
pub async fn check_member_of_group_or_app_role(
    user: User,
    group_id: Uuid,
    role: AppRole,
    db: &sqlx::PgPool,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            (SELECT COUNT(*) > 0 FROM account_group_memberships
             WHERE account_id = $1 AND group_id = $2)
            OR
            account_role_exists($1, $3)
        ) AS "allowed: bool""#,
        user.id,
        group_id,
        role as _,
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

/// Returns `Ok(())` if the user is a `manager`-role member of the given group
/// OR holds the given [`AppRole`]. Useful for routes that can be accessed by
/// either a site-wide staff role or the group's own manager.
pub async fn check_manager_of_group_or_app_role(
    user: User,
    group_id: Uuid,
    role: AppRole,
    db: &sqlx::PgPool,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            (SELECT COUNT(*) > 0 FROM account_group_memberships
             WHERE account_id = $1 AND role = 'manager' AND group_id = $2)
            OR
            account_role_exists($1, $3)
        ) AS "allowed: bool""#,
        user.id,
        group_id,
        role as _,
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

/// Axum middleware that gates a router behind a specific [`AppRole`].
///
/// Intended to be used via a closure passed to [`axum::middleware::from_fn_with_state`]:
///
/// ```ignore
/// .route_layer(middleware::from_fn_with_state(
///     app_state.clone(),
///     |state, ext, req, next| {
///         permissions::require_app_role(AppRole::NetworkManager, state, ext, req, next)
///     },
/// ))
/// ```
pub async fn require_app_role(
    role: AppRole,
    State(app_state): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    request: Request,
    next: Next,
) -> Result<Response, VialoError> {
    check_app_role(user, role, &app_state.db).await?;
    Ok(next.run(request).await)
}
