use crate::{
    AppState,
    health::add_health_event,
    helpers::{encryption::Encrypted, grab_authd_conn_subsystem},
    http::network::models::NmsType,
};
use anyhow::anyhow;
use serde_json::json;
use sqlx::{Executor, PgConnection, PgPool, Postgres, types::Json};
use std::sync::Arc;
use tokio::{sync::watch, time};
use tracing::{debug, error, info, warn};
mod models;
mod traits;
mod unifi;
use crate::http::history::models::Subsystem;
use models::*;
use traits::*;
use unifi::UniFiApi;

/// Pushes the PPSK list of every password network linked to the connector.
async fn sync_connector(
    api: &mut impl NmsApi,
    connector_id: i32,
    db_pool: &PgPool,
    conn: &mut PgConnection,
) -> Result<(), anyhow::Error> {
    let links = sqlx::query!(
        "SELECT nnl.nms_id, nnl.network_id FROM net_network_nms_link nnl
         JOIN net_networks n ON n.id = nnl.network_id
         WHERE nnl.nms_connector_id = $1 AND n.auth = 'password'",
        connector_id
    )
    .fetch_all(db_pool)
    .await?;

    for link in links {
        let mut entries = vec![];
        for p in sqlx::query!(
            r#"select nc.password as "password!: Encrypted<String>", nr.vlan as "vlan!", nc.id from net_cred nc JOIN net_realm_assignments nra ON nc.account_id = nra.account_ID JOIN net_realms nr ON nr.id = nra.realm_id WHERE nc.network_id = $1 AND nc.password IS NOT NULL AND nr.vlan IS NOT NULL"#,
            link.network_id
        )
        .fetch_all(db_pool)
        .await?
        {
            let passphrase = p.password.expose();
            if passphrase.len() < 8 || passphrase.len() > 63 {
                error!("Invalid password length. Ignoring password for cred \"{}\".", p.id);
                add_health_event(&mut *conn, Subsystem::Ppsk, "bad_password", Some(json!({"cred_id": p.id})), 2, false, None).await;
                continue;
            }
            entries.push(PpskEntry { passphrase, vlan: p.vlan });
        }

        for issue in api.set_ppsks(&link.nms_id, &entries).await? {
            match issue {
                NmsError::VlanUnavailable { vlan } => {
                    add_health_event(
                        &mut *conn,
                        Subsystem::Ppsk,
                        "vlan_unavailable",
                        Some(json!({"connector_id": connector_id, "vlan": vlan})),
                        2,
                        false,
                        None,
                    )
                    .await
                }
                NmsError::EmptyListUnsupported => {
                    add_health_event(
                        &mut *conn,
                        Subsystem::Ppsk,
                        "empty_list",
                        Some(json!({"network_id": link.network_id})),
                        50,
                        false,
                        None,
                    )
                    .await
                }
            }
        }
    }

    Ok(())
}

pub async fn add_task<'a, E>(db: E, job: JobData) -> Result<i32, sqlx::Error>
where
    E: Executor<'a, Database = Postgres>,
{
    sqlx::query_scalar!("INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status) VALUES ('ppsk',$1,NOW(),NOW(), 'pending') RETURNING id", Json(job) as _).fetch_one(db).await
}

pub async fn main(
    db_pool: PgPool,
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    loop {
        if *shutdown.borrow() {
            info!("Arrividerci!");
            break;
        }

        let mut current_job_id = None;

        let mut conn = grab_authd_conn_subsystem(&db_pool.clone(), "ppsk")
            .await
            .map_err(|e| anyhow::anyhow!("couldn't connect to db: {e:?}"))?;

        if let Some(job) = sqlx::query!(
        r#"UPDATE subsystem_jobs SET status = 'processing', last_updated = NOW() WHERE id IN (SELECT id
                             FROM subsystem_jobs
                             WHERE subsystem = 'ppsk' AND (status = 'pending' OR (status = 'error' AND last_updated < NOW() - INTERVAL '5 minutes')) ORDER BY id ASC
                             LIMIT 1) RETURNING id, data as "data: Json<JobData>", created_at, OLD.last_updated, OLD.status AS "status: models::JobStatus""#).fetch_optional(&mut *conn).await?
    {
        info!("Job {:?}", job);
        if job.status == JobStatus::Error {
            warn!("This is a retry. {}", job.last_updated.map(|l| format!("Last attempted {}", l)).unwrap_or("Never attempted before (huh?)".into()))
        }
        current_job_id = Some(job.id);
    }else{
        debug!("Nothing to do.");
        tokio::select! {
            _ = time::sleep(time::Duration::from_secs(30)) => {}
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    info!("Arrividerci");
                    break;
                }
            }
        }
    }
        if let Some(job_id) = current_job_id {
            let mut run_task = async || -> Result<(), anyhow::Error> {
                let connectors = sqlx::query!(r#"SELECT id, type as "type: NmsType", hostname, api_key as "api_key: Encrypted<String>", site_id FROM net_nms_connectors"#)
                    .fetch_all(&db_pool)
                    .await?;

                let mut failed = 0;
                for c in connectors {
                    let result = match c.r#type {
                        NmsType::Unifi => {
                            async {
                                let mut api = UniFiApi::new(
                                    c.hostname,
                                    c.api_key.ok_or_else(|| anyhow!("no API key"))?.expose(),
                                    c.site_id.ok_or_else(|| anyhow!("no site ID"))?,
                                    &app_state.config.proxy,
                                )?;
                                sync_connector(&mut api, c.id, &db_pool, &mut conn).await
                            }
                            .await
                        }
                        NmsType::Mikrotik => {
                            warn!(
                                "someone needs to implement mikrotik for connector {} to work",
                                c.id
                            );
                            continue;
                        }
                    };
                    if let Err(e) = result {
                        error!("Connector {} failed: {:?}", c.id, e);
                        failed += 1;
                    }
                }
                if failed > 0 {
                    return Err(anyhow!("{failed} connector(s) failed"));
                }
                Ok(())
            };

            let task = "Refresh";
            info!("Running task {:?}...", task);
            let startt = time::Instant::now();
            let status = match run_task().await {
                Ok(_) => {
                    let now = time::Instant::now();
                    info!(
                        "Ran task {:?} in {} sec",
                        task,
                        now.duration_since(startt).as_secs()
                    );

                    JobStatus::Done
                }
                Err(error) => {
                    error!("Error running task {:?}: {}", task, error);
                    JobStatus::Error
                }
            };

            sqlx::query!(
                "UPDATE subsystem_jobs SET status = $1, last_updated = NOW() WHERE id = $2",
                status as JobStatus,
                job_id
            )
            .execute(&mut *conn)
            .await?;
        }
    }

    Ok(())
}
