use sqlx::PgExecutor;
use uuid::Uuid;

use crate::http::util::VialoError;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type)]
#[sqlx(type_name = "bookable_perm", rename_all = "snake_case")]
pub enum BookablePerm {
    View,
    Book,
    Admin,
}

pub async fn require_asset_type_perm<'c, T: PgExecutor<'c>>(
    user_id: Uuid,
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
