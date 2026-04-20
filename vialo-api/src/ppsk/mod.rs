use crate::{
    AppState,
    health::add_health_event,
    helpers::{encryption::Encrypted, grab_authd_conn_subsystem},
};
use serde_json::json;
use sqlx::{Executor, PgPool, Postgres, types::Json};
use std::{collections::HashMap, sync::Arc};
use tokio::{sync::watch, time};
use tracing::{error, info, warn};
mod lib;
mod models;
use crate::http::history::models::Subsystem;
use lib::*;
use models::*;
pub async fn add_task<'a, E>(db: E, job: JobData) -> Result<i32, sqlx::Error>
where
    E: Executor<'a, Database = Postgres>,
{
    sqlx::query_scalar!("INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status) VALUES ('ppsk',$1,NOW(),NOW(), 'pending') RETURNING id", Json(job) as _).fetch_one(db).await
}

pub async fn main(
    db_pool: PgPool,
    _app_state: Arc<AppState>,
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
            .expect("couldn't connect to db!");

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
        info!("Nothing to do.");
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
                let credentials = sqlx::query!(r#"SELECT hostname, api_key as "api_key!: Encrypted<String>", site_id as "site_id!" FROM net_nms_connectors WHERE type = 'unifi'"#)
        .fetch_one(&db_pool)
        .await?;
                // https://i.kym-cdn.com/photos/images/newsfeed/002/425/217/e06
                let session = UniFiApi::new(
                    credentials.hostname,
                    credentials.api_key.expose(),
                    credentials.site_id,
                )?;

                let unifi_vlans: HashMap<i64, _> = session
                    .get_network_conf()
                    .await?
                    .into_iter()
                    .map(|n| (n.vlan_id, n))
                    .collect();

                let unifi_wlan_summaries: HashMap<String, _> = session
                    .get_wlans()
                    .await?
                    .into_iter()
                    .map(|u| (u.name.clone(), u))
                    .collect();

                // Look up the network by label (TODO: set up proper linking)
                let vialo_networks: Vec<_> =
                    sqlx::query!("SELECT id, label FROM net_networks WHERE auth = 'password'")
                        .fetch_all(&db_pool)
                        .await?
                        .into_iter()
                        .filter_map(|v| {
                            unifi_wlan_summaries
                                .get(&v.label)
                                .map(|u| (u.id.clone(), v.id))
                        })
                        .collect();

                for (wlan_summary_id, net_id) in vialo_networks {
                    let mut old_wlan_unifi = session.get_wlan(&wlan_summary_id).await?;
                    let mut ppsk: Vec<WlanPresharedKey> = vec![];
                    for p in sqlx::query!(
            r#"select nc.password as "password!: Encrypted<String>", nr.vlan as "vlan!", nc.id from net_cred nc JOIN net_realm_assignments nra ON nc.account_id = nra.account_ID JOIN net_realms nr ON nr.id = nra.realm_id WHERE nc.network_id = $1 AND nc.password IS NOT NULL AND nr.vlan IS NOT NULL"#,
            net_id
        ).fetch_all(&db_pool)
        .await? {
            let password = p.password.expose();
            if password.len() < 8 || password.len() > 63 {
                error!("Invalid password length. Ignoring password for cred \"{}\".", p.id);
                add_health_event(&mut *conn, Subsystem::Ppsk, "bad_password", Some(json!({"cred_id": p.id})), 2, false).await?;
                continue;
            }
            let networkconf_id = if let Some(unifi_vlan) = unifi_vlans.get(&p.vlan.into()) {
                unifi_vlan.id.clone()
            } else {
                return Err(anyhow::anyhow!("VLAN {} not found in UniFi", p.vlan));
            };
            ppsk.push(WlanPresharedKey {
                network: WlanPresharedKeyNetwork {
                    network_type: WlanPresharedKeyNetworkType::Specific,
                    network_id: Some(networkconf_id),
                },
                passphrase: password,
            });
        }
                    // UniFi doesn't like empty PPSK
                    if !ppsk.is_empty() {
                        add_health_event(
                            &mut *conn,
                            Subsystem::Ppsk,
                            "empty_list",
                            Some(json!({"network_id": net_id})),
                            50,
                            false,
                        )
                        .await?;

                        old_wlan_unifi.security_configuration.preshared_keys = Some(ppsk);
                        session.set_wlan(&old_wlan_unifi).await?;
                    }
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
