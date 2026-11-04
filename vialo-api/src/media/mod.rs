//! Uploads: storage layout, the image encode worker and the orphan cleanup.
//!
//! Layout under `media.dir`: `<id[0..2]>/<id>/`, holding `orig`, plus `<width>.avif` per
//! variant for images. The path depends only on the id, so attaching never touches the disk.

use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::anyhow;
use sqlx::{Acquire, types::Json};
use tokio::{sync::watch, time::Instant};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{AppState, helpers::grab_authd_conn_subsystem};
use models::{JobData, MediaContext};

pub mod detect;
pub mod models;
pub mod process;

pub const ORIGINAL: &str = "orig";
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);
/// Make sure not to set this too low, since it might belong to an upload in flight
const CLEANUP_GRACE: Duration = Duration::from_secs(3600);

/// Two-char prefix keeps any single dir small
pub fn media_dir(root: &Path, id: Uuid) -> PathBuf {
    let id = id.to_string();
    root.join(&id[..2]).join(id)
}

pub fn variant_file(width: i32) -> String {
    format!("{width}.avif")
}

/// Missing is fine; other failures are left for the cleanup
pub async fn remove_media_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => warn!("couldn't remove {}: {e}", dir.display()),
    }
}

pub async fn main(
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let db = &app_state.db;
    let config = &app_state.config.media;
    let root = &config.dir;
    let threads = config.encode_threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1))
            .unwrap_or(1)
            .max(1)
    });
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("media-{i}"))
            .build()?,
    );
    tokio::fs::create_dir_all(root).await?;

    // Single worker, so anything still processing was interrupted
    sqlx::query!("UPDATE subsystem_jobs SET status = 'pending', last_updated = NOW() WHERE subsystem = 'media' AND status = 'processing'")
        .execute(db)
        .await?;

    let mut next_cleanup = Instant::now();
    loop {
        if *shutdown.borrow() {
            info!("Arrividerci!");
            break;
        }

        if Instant::now() >= next_cleanup {
            // Cleanup: abandoned uploads, then any dir the DB doesn't know
            let cleanup = async {
                // Their dirs go in the pass below
                let mut conn = grab_authd_conn_subsystem(db, "media")
                    .await
                    .map_err(|e| anyhow!("couldn't connect to db: {e:?}"))?;
                let stale = sqlx::query!(
                    r#"DELETE FROM media m WHERE m.created_at < NOW() - interval '1 day' AND (
                        (m.context = 'post' AND m.post_id IS NULL)
                        OR (m.context = 'avatar' AND m.status <> 'pending'
                            AND NOT EXISTS (SELECT 1 FROM accounts_people ap WHERE ap.avatar_id = m.id)))"#
                )
                .execute(&mut *conn)
                .await?
                .rows_affected();
                if stale > 0 {
                    info!("Removed {stale} abandoned upload(s)");
                }

                // Media dirs on disk old enough to judge. One blocking hop for the scan
                let pattern_root = glob::Pattern::escape(
                    root.to_str()
                        .ok_or_else(|| anyhow!("media.dir is not UTF-8"))?,
                );
                let found: Vec<(PathBuf, Uuid)> = tokio::task::spawn_blocking(move || {
                    let mut found = vec![];
                    for entry in glob::glob(&format!("{pattern_root}/*/*"))? {
                        let path = match entry {
                            Ok(path) => path,
                            Err(e) => {
                                warn!(
                                    "Media cleanup can't read {}: {}",
                                    e.path().display(),
                                    e.error()
                                );
                                continue;
                            }
                        };
                        let Some(id) = path
                            .file_name()
                            .and_then(|n| n.to_str()?.parse::<Uuid>().ok())
                        else {
                            continue;
                        };
                        let old_enough = std::fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .is_ok_and(|t| {
                                SystemTime::now()
                                    .duration_since(t)
                                    .is_ok_and(|d| d > CLEANUP_GRACE)
                            });
                        if path.is_dir() && old_enough {
                            found.push((path, id));
                        }
                    }
                    Ok::<_, anyhow::Error>(found)
                })
                .await??;

                let ids: Vec<Uuid> = found.iter().map(|(_, id)| *id).collect();
                let known: HashSet<Uuid> =
                    sqlx::query_scalar!("SELECT id FROM media WHERE id = ANY($1)", &ids)
                        .fetch_all(db)
                        .await?
                        .into_iter()
                        .collect();

                for (path, id) in found {
                    if !known.contains(&id) || path != media_dir(root, id) {
                        debug!("Removing orphaned {}", path.display());
                        remove_media_dir(&path).await;
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            if let Err(e) = cleanup.await {
                warn!("Media cleanup failed: {e:?}");
            }
            next_cleanup = Instant::now() + CLEANUP_INTERVAL;
        }

        let job = sqlx::query!(
            r#"UPDATE subsystem_jobs SET status = 'processing', last_updated = NOW() WHERE id = (
                SELECT id FROM subsystem_jobs WHERE subsystem = 'media' AND status = 'pending'
                ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED)
            RETURNING id, data AS "data: Json<JobData>""#
        )
        .fetch_optional(db)
        .await?;

        let Some(job) = job else {
            tokio::select! {
                _ = app_state.media_jobs.notified() => {}
                _ = tokio::time::sleep_until(next_cleanup) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        info!("Arrividerci!");
                        break;
                    }
                }
            }
            continue;
        };

        let id = job.data.media_id;
        let started = Instant::now();

        // Encode one image
        let run = async {
            let Some(context) = sqlx::query_scalar!(
                r#"SELECT context AS "context: MediaContext" FROM media WHERE id = $1"#,
                id
            )
            .fetch_optional(db)
            .await?
            else {
                debug!("media {id} deleted before encoding");
                return Ok(());
            };
            let dir = media_dir(root, id);
            let bytes = tokio::fs::read(dir.join(ORIGINAL)).await?;

            let pool = pool.clone();
            let processed = tokio::task::spawn_blocking(move || {
                pool.install(|| process::process(&bytes, context))
            })
            .await??;

            let mut conn = grab_authd_conn_subsystem(db, "media")
                .await
                .map_err(|e| anyhow!("couldn't connect to db: {e:?}"))?;
            let mut trans = conn.begin().await?;

            // Deleted while encoding, WHATEVER
            let Some(owner_id) =
                sqlx::query_scalar!("SELECT owner_id FROM media WHERE id = $1 FOR UPDATE", id)
                    .fetch_optional(&mut *trans)
                    .await?
            else {
                return Ok(());
            };

            // Write-then-rename so readers never see a partial file
            for v in &processed.variants {
                let path = dir.join(variant_file(v.width as i32));
                let part = path.with_extension("avif.part");
                tokio::fs::write(&part, &v.avif).await?;
                tokio::fs::rename(&part, &path).await?;
            }

            let widths: Vec<i32> = processed.variants.iter().map(|v| v.width as i32).collect();
            sqlx::query!(
                "UPDATE media SET status = 'ready', width = $2, height = $3, variants = $4 WHERE id = $1",
                id,
                processed.width as i32,
                processed.height as i32,
                &widths
            )
            .execute(&mut *trans)
            .await?;

            // A finished avatar replaces the current one
            let mut replaced = None;
            if context == MediaContext::Avatar {
                let old = sqlx::query_scalar!(
                    "UPDATE accounts_people SET avatar_id = $1 WHERE id = $2 RETURNING OLD.avatar_id",
                    id,
                    owner_id
                )
                .fetch_optional(&mut *trans)
                .await?
                .flatten();
                if let Some(old) = old.filter(|old| *old != id) {
                    sqlx::query!("DELETE FROM media WHERE id = $1", old)
                        .execute(&mut *trans)
                        .await?;
                    replaced = Some(media_dir(root, old));
                }
            }

            trans.commit().await?;

            if let Some(dir) = replaced {
                remove_media_dir(&dir).await;
            }
            Ok::<_, anyhow::Error>(())
        };

        // Abandoning mid-encode is fine because the job is reset on next start
        let result = tokio::select! {
            result = run => result,
            _ = shutdown.changed() => {
                info!("Arrividerci! (abandoning media {id})");
                break;
            }
        };

        let ok = match result {
            Ok(()) => {
                debug!("Encoded media {id} in {:?}", started.elapsed());
                true
            }
            Err(e) => {
                error!("Encoding media {id} failed: {e:?}");
                let mut conn = grab_authd_conn_subsystem(db, "media")
                    .await
                    .map_err(|e| anyhow!("couldn't connect to db: {e:?}"))?;
                sqlx::query!("UPDATE media SET status = 'error' WHERE id = $1", id)
                    .execute(&mut *conn)
                    .await?;
                false
            }
        };
        sqlx::query!(
            "UPDATE subsystem_jobs SET status = CASE WHEN $2 THEN 'done'::job_status ELSE 'error'::job_status END, last_updated = NOW() WHERE id = $1",
            job.id,
            ok
        )
        .execute(db)
        .await?;
    }

    Ok(())
}
