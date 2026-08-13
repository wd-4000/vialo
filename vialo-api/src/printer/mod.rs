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
use sqlx::{Acquire, Executor, PgConnection, PgPool, Postgres, types::Json};
use std::{env, sync::Arc};
use tokio::{fs, sync::watch, time};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

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

/// Refreshes mirror rows from the device's counter list, returns the device's
/// current account ids.
async fn refresh_counters(
    p: &mut impl PrinterApi,
    connection: &mut PgConnection,
) -> Result<Vec<i32>, anyhow::Error> {
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

    Ok(ids)
}

/// Recreates a lost device account from the person's amenities login,
/// settling counters and pending ledger rows first. Returns the new printer_id.
async fn restore_printer_account(
    printer: &mut impl PrinterApi,
    conn: &mut PgConnection,
    account_id: Uuid,
) -> Result<u16, anyhow::Error> {
    let account = sqlx::query!(
        "SELECT email, amenities_username, amenities_pin from accounts_people where id = $1",
        account_id
    )
    .fetch_optional(&mut *conn)
    .await?;

    let (email, username, pin_encrypted) = match account {
        Some(account) => {
            match (
                account.email,
                account.amenities_username,
                account.amenities_pin,
            ) {
                (Some(email), Some(username), Some(pin)) => (email, username, pin),
                _ => {
                    let dedup = format!("cannot_restore_{account_id}");
                    add_health_event(
                        &mut *conn,
                        Subsystem::Printer,
                        "cannot_restore",
                        Some(json!({"account_id": account_id, "reason": "missing_credentials"})),
                        50,
                        false,
                        Some(&dedup),
                    )
                    .await;
                    anyhow::bail!(
                        "Cannot restore printer account {account_id}: missing email/username/PIN"
                    );
                }
            }
        }
        None => {
            let dedup = format!("cannot_restore_{account_id}");
            add_health_event(
                &mut *conn,
                Subsystem::Printer,
                "cannot_restore",
                Some(json!({"account_id": account_id, "reason": "account_deleted"})),
                50,
                false,
                Some(&dedup),
            )
            .await;
            anyhow::bail!("Cannot restore printer account {account_id}: person no longer exists");
        }
    };

    // Decrypt the PIN before passing it to the printer
    let pin: String = encryption::decrypt(&pin_encrypted)?;

    // Settle counters and pending ledger rows like FullSync does. Idempotent.
    let mut trans = conn.begin().await?;
    sqlx::query!(
        "UPDATE subsystem_printer_context SET bw = 0, color = 0 WHERE id = $1",
        account_id
    )
    .execute(&mut *trans)
    .await?;
    sqlx::query!(
        r#"UPDATE credit_ledger SET status = 'done' WHERE from_account = $1 AND status = 'pending' AND (product = 'printer_bw' OR product = 'printer_color')"#,
        account_id
    )
    .execute(&mut *trans)
    .await?;

    let new_printer_id = printer.create_user(email, username.clone(), pin).await?;

    // Point the mirror row at the new account
    sqlx::query!(
        "UPDATE subsystem_printer_context SET (printer_id, printer_username) = ($1, $2) WHERE id = $3",
        new_printer_id as i32,
        username,
        account_id
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    let dedup = format!("restore_{account_id}");
    add_health_event(
        &mut *conn,
        Subsystem::Printer,
        "account_restored",
        Some(json!({"account_id": account_id, "printer_id": new_printer_id})),
        70,
        false,
        Some(&dedup),
    )
    .await;

    Ok(new_printer_id)
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

            debug!("I'm gurch!");
            let now = time::Instant::now();
            let mut current_task = None;
            let mut current_job_id = None;
            let mut conn = grab_authd_conn_subsystem(&printer_subsystem_pool.clone(), "printer")
                .await
                .map_err(|e| anyhow::anyhow!("couldn't connect to db: {e:?}"))?;

            // Daily full sync, once per venue day after 04:00. The drain guard
            // defers the run while other jobs are pending.
            if sqlx::query_scalar!(
                r#"SELECT (now()::time >= TIME '04:00')
                       AND NOT EXISTS (
                           SELECT 1 FROM subsystem_jobs
                           WHERE subsystem = 'printer'
                             AND data ->> 'type' = 'full_sync'
                             AND created_at >= date_trunc('day', now())
                       ) AS "due!""#
            )
            .fetch_one(&mut *conn)
            .await?
            {
                info!("It's past 04:00 venue time; enqueueing daily FullSync");
                add_task(&mut *conn, JobData::FullSync {}).await?;
            }

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
                            debug!("Nothing to do.");
                            // Don't forget to unlock!!
                            if printer.printer_lock().is_some() {
                                printer.unlock().await.context("Couldn't unlock printer")?;
                                debug!("Unlocked");
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
                        JobData::Refresh {} => {
                            refresh_counters(&mut printer, &mut conn).await?;
                        }
                        JobData::UpdateAccountLimit { account_id } => {
                            let account_info = sqlx::query!(
                                "SELECT credit_balance, printer_id, printer_username, bw, color FROM accounts_people ap JOIN subsystem_printer_context spc ON ap.id = spc.id WHERE ap.id = $1",
                                account_id
                            )
                            .fetch_optional(&mut *conn)
                            .await?;

                            let Some(account_info) = account_info else {
                                warn!(
                                    "No printer context for account {}. Skipping limit update.",
                                    account_id
                                );
                                return Ok(());
                            };

                            // Get the current pricing info. NULL means no price is
                            // configured for that product, so we can't sell it.
                            let pricing = sqlx::query!(
                                r#"SELECT
                                    (SELECT unit_price FROM subsystem_printer_pricing
                                        WHERE product = 'printer_bw'
                                          AND begins <= NOW()
                                        ORDER BY begins DESC
                                        LIMIT 1) AS bw_price,
                                    (SELECT unit_price FROM subsystem_printer_pricing
                                        WHERE product = 'printer_color'
                                          AND begins <= NOW()
                                        ORDER BY begins DESC
                                        LIMIT 1) AS color_price"#
                            )
                            .fetch_one(&mut *conn)
                            .await?;

                            if pricing.bw_price.is_none() {
                                warn!("No printer_bw price configured, so it's free");
                            }
                            if pricing.color_price.is_none() {
                                warn!("No printer_color price configured, so it's free");
                            }

                            // Limits are counter + balance/price, the device only
                            // zeroes counters on FullSync. None price means free,
                            // NULL balance means unlimited.
                            let limit_for = |price: Option<i32>,
                                             counter: i32,
                                             balance: Option<i32>|
                             -> Option<u16> {
                                match (price, balance) {
                                    // Without a price we'll just assume it's free.
                                    (None, _) | (Some(..=0), _) => None,
                                    // NULL balance = unlimited credits
                                    (Some(_), None) => None,
                                    (Some(price), Some(balance)) => Some(
                                        (counter as i64 + balance.max(0) as i64 / price as i64)
                                            .min(u16::MAX as i64)
                                            as u16,
                                    ),
                                }
                            };

                            // Fetch the device username when the mirror lacks it.
                            // Restore the account when the device lost it.
                            let username = match account_info.printer_username {
                                Some(u) => u,
                                None => {
                                    warn!(
                                        "Printer username not provided for account {}. Trying to correct",
                                        account_id
                                    );
                                    match printer.get_username(account_info.printer_id as u16).await
                                    {
                                        Ok(found_username) => {
                                            sqlx::query!(
                                                "UPDATE subsystem_printer_context SET printer_username = $1 WHERE id = $2", found_username, account_id
                                            )
                                            .execute(&mut *conn)
                                            .await?;

                                            found_username
                                        }
                                        Err(err)
                                            if matches!(
                                                err.downcast_ref::<PrinterRequestError>(),
                                                Some(PrinterRequestError::AccountNotFound)
                                            ) =>
                                        {
                                            warn!(
                                                "Printer account {} is gone from the device. Restoring",
                                                account_id
                                            );
                                            let new_printer_id = restore_printer_account(
                                                &mut printer,
                                                &mut *conn,
                                                *account_id,
                                            )
                                            .await?;
                                            let restored_username = sqlx::query_scalar!(
                                                "SELECT printer_username FROM subsystem_printer_context WHERE id = $1",
                                                account_id
                                            )
                                            .fetch_one(&mut *conn)
                                            .await?
                                            .context("Restored printer account has no username")?;
                                            printer
                                                .set_user_limit(
                                                    new_printer_id,
                                                    restored_username.clone(),
                                                    limit_for(
                                                        pricing.color_price,
                                                        0,
                                                        account_info.credit_balance,
                                                    ),
                                                    limit_for(
                                                        pricing.bw_price,
                                                        0,
                                                        account_info.credit_balance,
                                                    ),
                                                )
                                                .await?;
                                            // We applied the limits already
                                            return Ok(());
                                        }
                                        Err(err) => return Err(err),
                                    }
                                }
                            };

                            match printer
                                .set_user_limit(
                                    account_info.printer_id as u16,
                                    username,
                                    limit_for(
                                        pricing.color_price,
                                        account_info.color,
                                        account_info.credit_balance,
                                    ),
                                    limit_for(
                                        pricing.bw_price,
                                        account_info.bw,
                                        account_info.credit_balance,
                                    ),
                                )
                                .await
                            {
                                Ok(_) => {}
                                Err(err)
                                    if matches!(
                                        err.downcast_ref::<PrinterRequestError>(),
                                        Some(PrinterRequestError::AccountNotFound)
                                    ) =>
                                {
                                    warn!(
                                        "Printer account {} is gone from the device; recreating and re-applying limit",
                                        account_id
                                    );
                                    let new_printer_id = restore_printer_account(
                                        &mut printer,
                                        &mut conn,
                                        *account_id,
                                    )
                                    .await?;
                                    // Re-read the username, the restored account
                                    // uses the amenities login.
                                    let restored_username = sqlx::query_scalar!(
                                            "SELECT printer_username FROM subsystem_printer_context WHERE id = $1",
                                            account_id
                                        )
                                        .fetch_one(&mut *conn)
                                        .await?
                                        .context("Restored printer account has no username")?;
                                    printer
                                        .set_user_limit(
                                            new_printer_id,
                                            restored_username,
                                            limit_for(
                                                pricing.color_price,
                                                0,
                                                account_info.credit_balance,
                                            ),
                                            limit_for(
                                                pricing.bw_price,
                                                0,
                                                account_info.credit_balance,
                                            ),
                                        )
                                        .await?;
                                }
                                Err(err) => return Err(err),
                            }
                        }
                        JobData::FullSync {} => {
                            // Defer while other jobs are pending. Re-enqueue a
                            // fresh job, the worker overwrites this job's status
                            // when it finishes. Idempotent, so pile-ups are
                            // harmless.
                            if sqlx::query_scalar!(
                                r#"SELECT EXISTS(
                                       SELECT 1 FROM subsystem_jobs
                                        WHERE subsystem = 'printer' AND status = 'pending'
                                   ) AS "exists!""#
                            )
                            .fetch_one(&mut *conn)
                            .await?
                            {
                                info!("FullSync deferred: pending printer jobs still queued.");
                                add_task(&mut *conn, JobData::FullSync {}).await?;
                                return Ok(());
                            }

                            // Do a refresh, clear all counters and commit all transactions
                            printer.lock().await?;
                            let device_ids = refresh_counters(&mut printer, &mut conn).await?;
                            // TODO check if it's possible to clear all counters with one request
                            // All mirror rows, including device mirrors without a person
                            let printer_users = sqlx::query!(
                                "SELECT id, printer_id FROM subsystem_printer_context"
                            )
                            .fetch_all(&mut *conn)
                            .await?;

                            // Settle person-linked rows, clearing the device counter
                            // only when the account still exists
                            for user in &printer_users {
                                let Some(account_id) = user.id else { continue };

                                let mut trans = grab_trans(&mut conn).await.unwrap();
                                let old_counters = sqlx::query!(
                                "UPDATE subsystem_printer_context SET bw = 0, color = 0 WHERE id = $1 RETURNING OLD.bw as bw, OLD.color as color",
                                account_id
                            ).fetch_one(&mut *trans).await?;

                                let printer_credit_sum = old_counters.bw + old_counters.color * 3;

                                let ledger_credit_sum = sqlx::query!(
                                r#"UPDATE credit_ledger SET status = 'done' WHERE from_account = $1 AND status = 'pending' AND (product = 'printer_bw' OR product = 'printer_color') RETURNING credits;"#,
                                account_id
                            ).fetch_all(&mut *trans).await?.into_iter().filter_map(|l| l.credits).sum::<i32>();

                                if printer_credit_sum != ledger_credit_sum {
                                    warn!(
                                        "Uncommitted printer transaction difference detected! \n
                                     User {} (Printer ID {}) \n
                                     Printer: {}, ledger: {}",
                                        account_id,
                                        user.printer_id,
                                        printer_credit_sum,
                                        ledger_credit_sum
                                    );
                                    add_health_event(
                                        &mut *trans,
                                        Subsystem::Printer,
                                        "transaction_difference",
                                        Some(json!({"account_id": account_id, "printer_id": user.printer_id, "printer_credit_sum": printer_credit_sum, "ledger_credit_sum":ledger_credit_sum})),
                                        100,
                                        false,
                                        None,
                                    )
                                    .await;
                                    //continue;
                                }

                                if device_ids.contains(&user.printer_id) {
                                    printer.clear_counter(user.printer_id as u16).await?;
                                } else {
                                    warn!(
                                        "Printer account {} (device id {}) missing on device; skipping counter clear",
                                        account_id, user.printer_id
                                    );
                                }

                                trans.commit().await?;
                            }

                            // Destroy rows whose device account no longer exists
                            for user in printer_users
                                .iter()
                                .filter(|u| !device_ids.contains(&u.printer_id))
                            {
                                match user.id {
                                    Some(account_id) => {
                                        // Person still linked, re-sync the account.
                                        add_task(&mut *conn, JobData::SyncAccount { account_id })
                                            .await?;
                                        let dedup = format!("missing_{account_id}");
                                        add_health_event(
                                            &mut *conn,
                                            Subsystem::Printer,
                                            "account_missing",
                                            Some(json!({"account_id": account_id, "printer_id": user.printer_id})),
                                            70,
                                            false,
                                            Some(&dedup),
                                        )
                                        .await;
                                    }
                                    None => {
                                        sqlx::query!(
                                            "DELETE FROM subsystem_printer_context WHERE printer_id = $1",
                                            user.printer_id
                                        )
                                        .execute(&mut *conn)
                                        .await?;
                                    }
                                }
                            }
                        }
                        JobData::SyncAccount { account_id } => {
                            let account = sqlx::query!(
                                "SELECT email, amenities_username, amenities_pin from accounts_people where id = $1",
                                account_id
                            )
                            .fetch_one(&mut *conn)
                            .await?;

                            let email = account.email.context("User must have email")?;
                            let username = account
                                .amenities_username
                                .context("User must have an amenities username")?;

                            // Decrypt the PIN before passing it to the printer
                            let pin: String = encryption::decrypt(
                                &account.amenities_pin.context("User must have a PIN")?,
                            )?;

                            let context = sqlx::query!(
                                "SELECT printer_id, printer_username FROM subsystem_printer_context WHERE id = $1",
                                account_id
                            )
                            .fetch_optional(&mut *conn)
                            .await?;

                            match context {
                                None => {
                                    let printer_id =
                                        printer.create_user(email, username.clone(), pin).await?;

                                    sqlx::query!("INSERT INTO subsystem_printer_context (id, printer_id, printer_username, bw, color) VALUES ($1,$2,$3,$4,$5)", account_id, printer_id as i32, username, 0,0).execute(&mut *conn).await?;
                                }
                                Some(context) => {
                                    let password_changed = match printer
                                        .set_user_password(
                                            context.printer_id as u16,
                                            username.clone(),
                                            pin,
                                        )
                                        .await
                                    {
                                        Ok(()) => true,
                                        // The device lost the account, recreate it.
                                        Err(err)
                                            if matches!(
                                                err.downcast_ref::<PrinterRequestError>(),
                                                Some(PrinterRequestError::AccountNotFound)
                                            ) =>
                                        {
                                            warn!(
                                                "Printer account {} (device id {}) is gone; recreating",
                                                account_id, context.printer_id
                                            );
                                            restore_printer_account(
                                                &mut printer,
                                                &mut *conn,
                                                *account_id,
                                            )
                                            .await?;
                                            false
                                        }
                                        Err(err) => return Err(err),
                                    };

                                    // The helper already wrote the mirror row when
                                    // it restored the account.
                                    if password_changed {
                                        if context.printer_username.as_deref() != Some(&username) {
                                            warn!(
                                                "Printer username for account {} drifted from {:?}, correcting",
                                                account_id, context.printer_username
                                            );
                                        }

                                        sqlx::query!("UPDATE subsystem_printer_context SET printer_username = $1 WHERE id = $2", username, account_id).execute(&mut *conn).await?;
                                    }
                                }
                            }
                        }
                        JobData::DeleteAccount { printer_id } => {
                            match printer.delete_user(*printer_id as u16).await {
                                Ok(_) => {}
                                // Already gone, treat as success
                                Err(err)
                                    if matches!(
                                        err.downcast_ref::<PrinterRequestError>(),
                                        Some(PrinterRequestError::AccountNotFound)
                                    ) =>
                                {
                                    warn!(
                                        "Printer account {} already gone from device; treating delete as success",
                                        printer_id
                                    );
                                }
                                Err(err) => return Err(err),
                            }
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
                        let dedup_key = task.as_ref();
                        let error_json = error
                            .downcast_ref::<PrinterRequestError>()
                            .map(|e| serde_json::to_value(e).unwrap_or_default())
                            .unwrap_or_else(|| json!(format!("{error}")));
                        add_health_event(
                            &mut *conn,
                            Subsystem::Printer,
                            "task_error",
                            Some(json!({"task": task, "error": error_json})),
                            50,
                            false,
                            Some(dedup_key),
                        )
                        .await;
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
