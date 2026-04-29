use crate::http::util::VialoError;
use serde::{self, Serialize};
use sqlx::{PgPool, Postgres, pool::PoolConnection};
use std::path::PathBuf;
use utoipa::ToSchema;

pub mod encryption;
mod i18n;
mod patch_option;
pub mod people;
mod pg_date;
mod pg_date_time;
pub use {i18n::*, patch_option::*, pg_date::*, pg_date_time::*};

pub fn find_upward(filename: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(filename);
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Utility enum for handling response variants while keeping type safety
/// Didn't feel like adding ``either``
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum ResponseVariant<A, B> {
    A(A),
    B(B),
}

/// Utility enum for handling language variants (all languages vs. localized).
/// basically the same as ResponseVariant, here for semantical reasons.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum LangVariant<A, L> {
    AllLangs(A),
    Localized(L),
}

pub async fn grab_authd_conn_subsystem(
    db: &PgPool,
    subsystem_name: &str,
) -> Result<PoolConnection<Postgres>, VialoError> {
    let mut conn = db.acquire().await?;

    sqlx::query!(
        "SELECT set_config('app.subsystem', $1, false)",
        subsystem_name
    )
    .fetch_optional(&mut *conn)
    .await?;

    Ok(conn)
}
