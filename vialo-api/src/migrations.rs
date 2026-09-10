use sqlx::{
    PgPool,
    migrate::{Migrate, Migrator},
};
use std::collections::HashMap;
use tracing::warn;

/** Rewrite stored checksums for migrations whose file changed after they were
applied.*/
pub async fn repair_checksums(pool: &PgPool, migrator: &Migrator) -> Result<usize, anyhow::Error> {
    let mut conn = pool.acquire().await?;
    // So that a fresh database is a no-op instead of an error.
    conn.ensure_migrations_table(&migrator.table_name).await?;

    let applied: HashMap<i64, Vec<u8>> = conn
        .list_applied_migrations(&migrator.table_name)
        .await?
        .into_iter()
        .map(|migration| (migration.version, migration.checksum.into_owned()))
        .collect();

    let mut repaired = 0;
    for migration in migrator.iter() {
        if migration.migration_type.is_down_migration() {
            continue;
        }
        let Some(checksum) = applied.get(&migration.version) else {
            continue;
        };
        if checksum[..] == migration.checksum[..] {
            continue;
        }

        sqlx::query!(
            "UPDATE _sqlx_migrations SET checksum = $1 WHERE version = $2",
            &migration.checksum[..],
            migration.version
        )
        .execute(&mut *conn)
        .await?;

        warn!(
            "Repaired checksum for migration {} ({})",
            migration.version, migration.description
        );
        repaired += 1;
    }

    Ok(repaired)
}
