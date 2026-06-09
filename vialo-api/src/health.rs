use serde::Serialize;
use sqlx::PgExecutor;

use crate::http::history::models::Subsystem;

pub async fn add_health_event<'e, T: Serialize>(
    database: impl PgExecutor<'e>,
    subsystem: Subsystem,
    label: &str,
    data: Option<T>,
    badness: i32,
    resolved: bool,
) {
    if let Err(e) = sqlx::query!(
        "INSERT INTO health_events (subsystem, data, label, badness, resolved) VALUES ($1,$2,$3,$4,$5)",
        subsystem as Subsystem,
        sqlx::types::Json(data) as _,
        label,
        badness,
        resolved
    )
    .execute(database)
    .await
    {
        tracing::error!("Failed to write health event '{label}': {e:?}");
    }
}
