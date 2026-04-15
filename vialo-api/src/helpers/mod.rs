use crate::http::util::VialoError;
use sqlx::{PgPool, Postgres, pool::PoolConnection};

pub mod people;
mod pg_date;
mod pg_date_time;
pub use {pg_date::*, pg_date_time::*};

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

    return Ok(conn);
}
