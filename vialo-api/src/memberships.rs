use std::{sync::Arc, time::Duration};

use sqlx::query;
use tokio::sync::watch;
use tracing::debug;

use crate::AppState;

/// Time until the next midnight in the DB timezone
async fn next_wakeup(db: &sqlx::PgPool) -> Result<Duration, sqlx::Error> {
    let secs = sqlx::query_scalar!(
        r#"SELECT EXTRACT(EPOCH FROM date_trunc('day', now()) + interval '1 day' - now())::float8 AS "secs!""#
    )
    .fetch_one(db)
    .await?;
    Ok(Duration::from_secs_f64(secs.max(0.0)) + Duration::from_secs(5))
}

/// Leases starting/ending don't write anything, so the triggers can't catch them, so we refresh daily.
pub async fn main(
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    loop {
        // recompute derived membership
        query!(r#"SELECT 1 AS "ok!" FROM refresh_derived_memberships()"#)
            .fetch_one(&app_state.db)
            .await?;

        debug!("Derived group memberships refreshed");

        let wait = next_wakeup(&app_state.db).await?;
        debug!(
            "Next derived group membership refresh in {}s",
            wait.as_secs()
        );

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
        }
    }

    Ok(())
}
