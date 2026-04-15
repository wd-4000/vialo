use serde::Serialize;
use sqlx::{Executor, PgExecutor};

use crate::http::history::models::Subsystem;

pub async fn add_health_event<'e, T: Serialize>(
    database: impl PgExecutor<'e>,
    subsystem: Subsystem,
    label: &str,
    data: Option<T>,
    badness: i32,
    resolved: bool,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "INSERT INTO health_events (subsystem, data, label, badness, resolved) VALUES ($1,$2,$3,$4,$5)",
        subsystem as Subsystem,
        sqlx::types::Json(data) as _,
        label,
        badness,
        resolved
    )
    .execute(database)
    .await?;
    Ok(())
}
