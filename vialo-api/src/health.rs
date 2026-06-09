use serde::Serialize;
use sqlx::PgExecutor;

use crate::http::history::models::Subsystem;

/// Upserts a health event. If `dedup_key` is set and an unresolved event with the
/// same (subsystem, label, dedup_key) already exists, bumps `occurrences` and
/// updates `last_updated` and `data` instead of inserting a duplicate. No deduplication when When `dedup_key` is `None.
pub async fn add_health_event<'e, T: Serialize>(
    database: impl PgExecutor<'e>,
    subsystem: Subsystem,
    label: &str,
    data: Option<T>,
    badness: i32,
    resolved: bool,
    dedup_key: Option<&str>,
) {
    if let Err(e) = sqlx::query!(
        r#"INSERT INTO health_events (subsystem, data, label, badness, resolved, last_updated, dedup_key)
           VALUES ($1, $2, $3, $4, $5, NOW(), $6)
           ON CONFLICT (subsystem, label, dedup_key) WHERE resolved = false
           DO UPDATE SET last_updated = NOW(), data = EXCLUDED.data, occurrences = health_events.occurrences + 1"#,
        subsystem as Subsystem,
        sqlx::types::Json(data) as _,
        label,
        badness,
        resolved,
        dedup_key
    )
    .execute(database)
    .await
    {
        tracing::error!("Failed to write health event '{label}': {e:?}");
    }
}
