use crate::AppState;
use crate::{
    health::add_health_event,
    helpers::encryption::{self},
    helpers::grab_authd_conn_subsystem,
    http::history::models::Subsystem,
    http::util::grab_trans,
};
use anyhow::Context;
use serde_json::json;
use sqlx::{Executor, PgConnection, PgPool, Postgres, query_scalar, types::Json};
use std::{env, sync::Arc};
use tokio::{fs, sync::watch, time};
use tracing::{error, info, warn};

#[cfg(feature = "printer_km")]
mod km;
#[cfg(feature = "printer_km")]
use km::KmPrinterApi;

pub mod models;
pub mod traits;
use models::*;
use traits::*;

pub async fn add_task<'a, E>(db: E, job: JobData) -> Result<i32, sqlx::Error>
where
    E: Executor<'a, Database = Postgres>,
{
    sqlx::query_scalar!("INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status) VALUES ('printer',$1,NOW(),NOW(), 'pending') RETURNING id", Json(job) as _).fetch_one(db).await
}

async fn refresh_usernames(
    p: &mut impl PrinterApi,
    connection: &mut PgConnection,
) -> Result<(), anyhow::Error> {
    let (ids, usernames) = p.get_username_list().await?;

    sqlx::query!(
        "UPDATE subsystem_printer_context
           SET printer_username = data.printer_username
           FROM (
               SELECT UNNEST($1::integer[]) AS printer_id,
                      UNNEST($2::text[]) AS printer_username
           ) AS data
           WHERE subsystem_printer_context.printer_id = data.printer_id",
        ids.as_slice(),
        usernames.as_slice()
    )
    .execute(&mut *connection)
    .await
    .context("Couldn't update printer usernames")?;

    Ok(())
}

async fn refresh_counters(
    p: &mut impl PrinterApi,
    connection: &mut PgConnection,
) -> Result<(), anyhow::Error> {
    let (ids, colors, bws) = p.get_counter_list().await?;

    sqlx::query!(
        "INSERT INTO subsystem_printer_context (printer_id, bw, color)
            SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::integer[])
            AS t(printer_id, bw, color)
            ON CONFLICT (printer_id)
            DO UPDATE SET
                bw = EXCLUDED.bw,
                color = EXCLUDED.color;",
        ids.as_slice(),
        bws.as_slice(),
        colors.as_slice()
    )
    .execute(&mut *connection)
    .await
    .context("Couldn't update printer counters")?;

    if sqlx::query_scalar!(
        "SELECT COUNT(*) > 0 FROM subsystem_printer_context WHERE printer_username IS NULL"
    )
    .fetch_optional(&mut *connection)
    .await?
    .flatten()
    .unwrap_or(false)
    {
        warn!("Found accounts with no username. Correcting.");
        refresh_usernames(p, connection).await?;
    }

    Ok(())
}

pub async fn main(
    printer_subsystem_pool: PgPool,
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    #[cfg(not(feature = "printer_km"))]
    {
        warn!(
            "Printer subsystem started but no backends enabled. Nothing to do. Go write a backend maybe."
        );
        while !*shutdown.borrow() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }
        return Ok(());
    }

    #[cfg(feature = "printer_km")]
    {
        // Check whether we left the printer locked
        let printer_lock = match fs::read("~printer.lock").await {
            Ok(contents) => Some(String::from_utf8_lossy(&contents).into()),
            Err(_) => Option::None,
        };

        let mut printer = KmPrinterApi::new(
            env::var("PRINTER_URL").expect("No printer URL provided!"),
            env::var("PRINTER_PASSWORD").expect("No printer password provided!"),
            printer_lock.clone(),
            &app_state.config.proxy,
        );
        info!("Connecting to printer...");
        match printer.login().await {
            Ok(_) => {
                info!("Connected to the printer!");
            }
            Err(e) => {
                error!("Failed to connect to printer: {:?}", e);
            }
        }

        // Lift printer lock
        if printer_lock.is_some() {
            match printer.unlock().await {
                Ok(_) => {
                    info!("Printer unlocked.");
                }
                Err(e) => {
                    error!("Failed to unlock printer: {:?}", e);
                }
            }
        }

        let mut last_refresh: Option<_> = None;
        loop {
            if *shutdown.borrow() {
                info!("Shutting down");
                if printer_lock.is_some() {
                    match printer.unlock().await {
                        Ok(_) => {
                            info!("Printer unlocked.");
                        }
                        Err(e) => {
                            error!("Failed to unlock printer: {:?}", e);
                        }
                    }
                }
                info!("See you later.");
                break;
            }

            info!("I'm gurch!");
            let now = time::Instant::now();
            let mut current_task = None;
            let mut current_job_id = None;
            let mut conn = grab_authd_conn_subsystem(&printer_subsystem_pool.clone(), "printer")
                .await
                .expect("couldn't connect to db!");

            // See what there is to do
            if last_refresh.is_none()
                || (now.duration_since(last_refresh.unwrap()).as_secs() > 60 * 5)
            {
                // Periodic printer sync
                current_task = Some(JobData::Refresh {});
                last_refresh = Some(now);
            } else {
                if let Some(job) = sqlx::query_as!(
                      JobModel,
                        r#"UPDATE subsystem_jobs SET status = 'processing', last_updated = NOW() WHERE id IN (SELECT id
                                     FROM subsystem_jobs
                                     WHERE subsystem = 'printer' AND (status = 'pending' OR (status = 'error' AND last_updated < NOW() - INTERVAL '5 minutes')) ORDER BY id ASC
                                     LIMIT 1) RETURNING id, data as "data: Json<JobData>", created_at, OLD.last_updated, OLD.status AS "status: JobStatus""#
                        )
                        .fetch_optional(&mut *conn)
                        .await?{
                            info!("Job {:?}", job);
                            if job.status == JobStatus::Error {
                                warn!("This is a retry. {}", job.last_updated.map(|l| format!("Last attempted {}", l)).unwrap_or("Never attempted before (huh?)".into()))
                            }
                            current_task = Some(job.data.as_ref().clone());
                            current_job_id = Some(job.id);
                        }else{
                            info!("Nothing to do.");
                            // Don't forget to unlock!!
                            if printer.printer_lock().is_some() {
                                printer.unlock().await.context("Couldn't unlock printer")?;
                                info!("Unlocked");
                            }

                            tokio::select! {
                                _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {}
                                changed = shutdown.changed() => {
                                    if changed.is_ok() && *shutdown.borrow() {
                                        info!("Printer subsystem shutting down.");
                                        break;
                                    }
                                }
                            }
                        }
            }

            // Do the thing we need to do
            if let Some(task) = current_task {
                let mut run_printer_task = async || -> Result<(), anyhow::Error> {
                    match &task {
                        JobData::Refresh {} => refresh_counters(&mut printer, &mut conn).await?,
                        JobData::UpdateAccountLimit {
                            account_id,
                            color_limit,
                            bw_limit,
                        } => {
                            let account_info = sqlx::query!(
                            "SELECT printer_id, printer_username FROM subsystem_printer_context WHERE id = $1", account_id
                        )
                        .fetch_one(&mut *conn)
                        .await?;

                            let username = match account_info.printer_username {
                                Some(u) => u,
                                None => {
                                    warn!(
                                        "Printer username not provided for account {}. Trying to correct",
                                        account_id
                                    );
                                    let found_username = printer
                                        .get_username(account_info.printer_id as u16)
                                        .await?;
                                    sqlx::query!(
                                    "UPDATE subsystem_printer_context SET printer_username = $1 WHERE id = $2", found_username, account_id
                                )
                                .execute(&mut *conn)
                                .await?;

                                    found_username
                                }
                            };

                            printer
                                .set_user_limit(
                                    account_info.printer_id as u16,
                                    username,
                                    *color_limit,
                                    *bw_limit,
                                )
                                .await?;
                        }
                        JobData::FullSync {} => {
                            // Do a refresh, clear all counters and commit all transactions
                            printer.lock().await?;
                            refresh_counters(&mut printer, &mut conn).await?;
                            // TODO check if it's possible to clear all counters with one request
                            let printer_users =
                            sqlx::query!(r#"SELECT id as "id!", printer_id FROM subsystem_printer_context WHERE id IS NOT NULL"#)
                                .fetch_all(&mut *conn)
                                .await?;

                            for user in printer_users {
                                let mut trans = grab_trans(&mut conn).await.unwrap();
                                let old_counters = sqlx::query!(
                                "UPDATE subsystem_printer_context SET bw = 0, color = 0 WHERE id = $1 RETURNING OLD.bw as bw, OLD.color as color",
                                user.id
                            ).fetch_one(&mut *trans).await?;

                                let printer_credit_sum = old_counters.bw + old_counters.color * 3;

                                let ledger_credit_sum = sqlx::query!(
                                r#"UPDATE credit_ledger SET status = 'done' WHERE from_account = $1 AND status = 'pending' AND (product = 'printer_bw' OR product = 'printer_color') RETURNING credits;"#,
                                user.id
                            ).fetch_all(&mut *trans).await?.into_iter().map(|l| l.credits).sum::<i32>();

                                if printer_credit_sum != ledger_credit_sum {
                                    warn!(
                                        "Uncommitted printer transaction difference detected! \n
                                     User {} (Printer ID {}) \n
                                     Printer: {}, ledger: {}",
                                        user.id,
                                        user.printer_id,
                                        printer_credit_sum,
                                        ledger_credit_sum
                                    );
                                    add_health_event(
                                        &mut *trans,
                                        Subsystem::Printer,
                                        "transaction_difference",
                                        Some(json!({"account_id": user.id, "printer_id": user.printer_id, "printer_credit_sum": printer_credit_sum, "ledger_credit_sum":ledger_credit_sum})),
                                        100,
                                        false,
                                    )
                                    .await?;
                                    //continue;
                                }

                                printer.clear_counter(user.printer_id as u16).await?;

                                trans.commit().await?;
                            }
                        }
                        JobData::CreateAccount {
                            account_id,
                            username,
                            password: password_encrypted,
                        } => {
                            let email = query_scalar!(
                                "SELECT email from accounts_people where id = $1",
                                account_id
                            )
                            .fetch_one(&mut *conn)
                            .await?
                            .context("User must have email")?;

                            // Decrypt the password before passing it to the printer
                            // A bit ugly yeah but I didn't want to reëncrypt it
                            let password: String = encryption::decrypt(password_encrypted)?;
                            let printer_id = printer
                                .create_user(email, username.clone(), password)
                                .await?;

                            sqlx::query!("INSERT INTO subsystem_printer_context (id, printer_id, printer_username, printer_password, bw, color) VALUES ($1,$2,$3,$4,$5,$6)", account_id, printer_id as i32, username, password_encrypted, 0,0).execute(&mut *conn).await?;
                        }
                        JobData::DeleteAccount { printer_id } => {
                            printer.delete_user(*printer_id as u16).await?;
                            sqlx::query!(
                                "DELETE from subsystem_printer_context where printer_id = $1",
                                printer_id
                            )
                            .execute(&mut *conn)
                            .await?;
                        }
                    }
                    Ok(())
                };
                info!("Running task {:?}...", task);
                let startt = time::Instant::now();
                let status = match run_printer_task().await {
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

                if let Some(id) = current_job_id {
                    sqlx::query!(
                        "UPDATE subsystem_jobs SET status = $1, last_updated = NOW() WHERE id = $2",
                        status as JobStatus,
                        id
                    )
                    .execute(&mut *conn)
                    .await?;
                }
            }
        }
        Ok(())
    }
}
