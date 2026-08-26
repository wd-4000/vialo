mod channel;
pub use channel::{BookableChannel, BookableQueueChannel};

use crate::helpers::{PgDateTime, encryption::Encrypted};
use crate::http::bookables::connectors::BookableConnectorWithPassword;
use crate::http::bookables::models::{
    BookableAssetQueue, BookableAssetStatus, BookableQueueEntry, BookableStatus,
};
use crate::http::util::grab_trans;
use crate::{AppState, helpers::grab_authd_conn_subsystem};
use netio::models::OutputPost;
use sqlx::{postgres::PgListener, query, query_as, query_scalar};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time;
use tracing::debug;

mod models;

mod netio;
pub use models::*;

pub const QUEUE_PREVIOUS_DEPTH: i64 = 2;
pub const QUEUE_UPCOMING_DEPTH: i64 = 4;

/// bookable_asset_queue
pub struct QueueRow {
    pub asset_id: i32,
    pub asset_type_id: i32,
    pub appointment_id: uuid::Uuid,
    pub begins: Option<chrono::DateTime<chrono::Utc>>,
    pub ends: Option<PgDateTime>,
    pub room: Option<String>,
    pub maintenance: bool,
    pub bucket: String,
}

pub fn assemble_queues(rows: Vec<QueueRow>) -> Vec<BookableAssetQueue> {
    let mut queues: HashMap<i32, BookableAssetQueue> = HashMap::new();
    for row in rows {
        let entry = BookableQueueEntry {
            appointment_id: row.appointment_id,
            begins: row.begins,
            ends: row.ends,
            room: row.room,
            maintenance: row.maintenance,
        };
        let queue = queues
            .entry(row.asset_id)
            .or_insert_with(|| BookableAssetQueue {
                id: row.asset_id,
                asset_type_id: row.asset_type_id,
                previous: Vec::new(),
                current: None,
                upcoming: Vec::new(),
            });
        match row.bucket.as_str() {
            "previous" => queue.previous.push(entry),
            "current" => queue.current = Some(entry),
            _ => queue.upcoming.push(entry),
        }
    }

    let mut queues: Vec<BookableAssetQueue> = queues.into_values().collect();
    queues.sort_by_key(|q| q.id);
    queues
}

pub async fn sync_connectors(app_state: &AppState) -> Result<(), anyhow::Error> {
    let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let connectors = query_as!(
        BookableConnectorWithPassword,
        r#"SELECT id,
        endpoint,
        num_outputs,
        device_name,
        serial_number,
        mac::text,
        username as "username!: Encrypted<String>",
        password as "password!: Encrypted<String>"
    FROM bookable_connectors;"#
    )
    .fetch_all(&app_state.db)
    .await?;

    for connector in connectors {
        let statuses = query_as!(
            BookableAssetStatusWithConnector,
            r#"SELECT id as "id!",
            status as "status!: BookableStatus",
            begins,
            ends as "ends: PgDateTime",
            connector as "connector!",
            connector_output_id as "connector_output_id!"
        FROM bookable_asset_status
        WHERE connector = $1 AND connector_output_id IS NOT NULL"#,
            connector.id
        )
        .fetch_all(&app_state.db)
        .await
        .unwrap();

        let outputs: Vec<OutputPost> = statuses
            .into_iter()
            .map(|s| OutputPost {
                id: s.connector_output_id,
                action: match s.status {
                    BookableStatus::Active | BookableStatus::QuickUnlock => {
                        netio::models::Action::On
                    }
                    _ => netio::models::Action::Off,
                },
            })
            .collect();

        let mut api = netio::NetioApi::new(
            connector.endpoint,
            connector.username.expose(),
            connector.password.expose(),
            &app_state.config.proxy,
        );

        let res = api
            .post()
            .json(&netio::models::NetioPost { outputs })
            .send()
            .await?
            .json::<netio::models::NetioGet>()
            .await?;

        query!(
        "UPDATE bookable_connectors SET num_outputs = $1, device_name = $2, serial_number = $3, mac = $4::macaddr WHERE id = $5",
        res.agent.num_outputs,
        res.agent.device_name,
        res.agent.serial_number,
        res.agent.mac as String,
        connector.id
    ).execute(&mut *conn).await?;
    }

    Ok(())
}

pub async fn auto_activate_appointments(app_state: &AppState) -> Result<(), anyhow::Error> {
    let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    query!(
        "UPDATE bookable_appointments ba
    SET activated = NOW()
    WHERE ba.activated IS NULL
    AND ba.cancelled_at IS NULL
    AND ba.maintenance = false
    AND lower(ba.during) <= now()
    AND NOT (
        SELECT bs.requires_activation
        FROM bookable_schema_assignments bsa
        JOIN bookable_schemas bs ON bs.id = bsa.schema_id
        WHERE bsa.asset_id = ba.asset_id
          AND bsa.begins <= lower(ba.during)
        ORDER BY bsa.begins DESC
        LIMIT 1
    )
"
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

pub async fn expire_appointments(app_state: &AppState) -> Result<(), anyhow::Error> {
    let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // Expire appointments.
    // Requires activation, not activated, not canceled, not maintenance, and grace period passed
    let expired_appointments = query!(
        r#"SELECT ba.id,
    ba.asset_id,
    ba.account_id,
    ba.transaction_id,
    cl.from_account,
    cl.credits,
    ba.cancelled_at,
    ba.cancellation_reason as "cancellation_reason: CancellationReason",
    ba.activated,
    cs.expiry_refund_percent as "expiry_refund_percent!",
    ap.full_name,
    ap.email,
    get_i18n_string(bast.name_i18n, $1) as "asset_name?"
    FROM bookable_appointments ba
    CROSS JOIN LATERAL (
        SELECT DISTINCT ON (bsa.asset_id)
            bsa.asset_id,
            bs.activation_grace_period,
            bs.expiry_refund_percent,
            bs.requires_activation
        FROM bookable_schema_assignments bsa
        JOIN bookable_schemas bs ON bs.id = bsa.schema_id
        WHERE bsa.asset_id = ba.asset_id
          AND bsa.begins <= lower(ba.during)
        ORDER BY bsa.asset_id, bsa.begins DESC
    ) cs
    JOIN credit_ledger cl ON ba.transaction_id = cl.id
    JOIN accounts_people ap ON ap.id = ba.account_id
    JOIN bookable_assets bast ON bast.id = ba.asset_id
    WHERE ba.activated IS NULL AND cancelled_at IS NULL AND ba.maintenance = false AND cs.requires_activation AND cs.expiry_refund_percent IS NOT NULL AND greatest(lower(ba.during), ba.created_at) + cs.activation_grace_period <= NOW()"#,
        &["en".into(), "de".into()],
    )
    .fetch_all(&app_state.db)
    .await?;

    for appointment in expired_appointments {
        let mut trans = grab_trans(&mut conn)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;

        query!(
            "UPDATE bookable_appointments SET cancelled_at = now(), cancellation_reason = 'expired' WHERE id = $1 AND cancelled_at IS NULL AND activated IS NULL",
            appointment.id,
        ).execute(&mut *trans).await?;
        let refund_credits =
            appointment.credits.unwrap_or(0) * appointment.expiry_refund_percent as i32 / 100;
        if refund_credits > 0 {
            query!(
                "INSERT INTO credit_ledger (to_account, refund_of, credits) VALUES ($1, $2, $3)",
                appointment.from_account,
                appointment.transaction_id,
                refund_credits
            )
            .execute(&mut *trans)
            .await?;
        }
        trans.commit().await?;

        let Some(email) = appointment.email else {
            continue;
        };

        if app_state
            .event_channels
            .expired_appointments_tx
            .send(ExpiredAppointmentMessage {
                id: appointment.id,
                account_id: appointment.account_id,
                full_name: appointment.full_name,
                email,
                asset_name: appointment.asset_name.unwrap_or_else(|| "Unknown".into()),
                credits_refunded: refund_credits,
            })
            .is_err()
        {
            tracing::error!(
                appointment_id = %appointment.id,
                "expired_appointments channel has no receivers — expiry email will not be sent"
            );
            crate::health::add_health_event(
                &app_state.db,
                crate::http::history::models::Subsystem::Email,
                "channel_no_receivers",
                Some(serde_json::json!({"appointment_id": appointment.id})),
                50,
                false,
                None,
            )
            .await;
        }
    }

    Ok(())
}

async fn next_wakeup(app_state: &AppState) -> tokio::time::Instant {
    let fallback = tokio::time::Instant::now() + Duration::from_secs(3600); // wake up every hour just in case

    let result = query_scalar!(
        r#"SELECT LEAST(
        -- the end of whatever is running now: nothing else wakes us for it, and
        -- both the status and the queue go stale the moment it passes
        (SELECT MIN(upper(ba.during))
           FROM bookable_appointments ba
          WHERE ba.cancelled_at IS NULL
            AND ba.during @> now()
            AND NOT upper_inf(ba.during)
            AND upper(ba.during) <> 'infinity'::timestamptz),
        -- quick unlocks are ranges too, and end the same way
        (SELECT MIN(upper(ba.quick_unlock))
           FROM bookable_assets ba
          WHERE ba.quick_unlock @> now()
            AND NOT upper_inf(ba.quick_unlock)),
        (SELECT MIN(
            CASE
                WHEN lower(ba.during) > now() THEN lower(ba.during) -- not yet started
                ELSE greatest(lower(ba.during), ba.created_at) + cs.activation_grace_period -- expired
            END
        )
        FROM bookable_appointments ba

        -- schema for asset
        LEFT JOIN (
            SELECT DISTINCT ON (bsa.asset_id)
                bsa.asset_id,
                bs.activation_grace_period,
                bs.expiry_refund_percent,
                bs.requires_activation
            FROM bookable_schema_assignments bsa
            JOIN bookable_schemas bs ON bs.id = bsa.schema_id
            WHERE bsa.begins <= now()
            ORDER BY bsa.asset_id, bsa.begins DESC
        ) cs ON cs.asset_id = ba.asset_id

        -- not activated, not canceled
        WHERE ba.cancelled_at IS NULL
        AND ba.activated IS NULL

        -- and begins or expires in the future
        AND (
            lower(ba.during) > now()
            OR (
                cs.requires_activation
                AND cs.activation_grace_period IS NOT NULL
                AND cs.expiry_refund_percent IS NOT NULL
                AND greatest(lower(ba.during), ba.created_at) + cs.activation_grace_period > now()
            )
        ))
        )::timestamptz"#
    )
    .fetch_one(&app_state.db)
    .await
    .ok()
    .flatten();

    let Some(ts) = result else {
        return fallback;
    };

    let duration_until = (ts - chrono::Utc::now()).to_std().unwrap_or(Duration::ZERO);
    tokio::time::Instant::now() + duration_until
}

pub async fn main(
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let mut listener = PgListener::connect_with(&app_state.db).await?;
    listener.listen("bookable_update").await?;

    // Empty on first pass, so everything broadcasts once
    let mut cache: HashMap<i32, (BookableAssetStatus, BookableAssetQueue)> = HashMap::new();

    loop {
        auto_activate_appointments(&app_state).await?;
        expire_appointments(&app_state).await?;
        sync_connectors(&app_state).await?;
        publish_bookables(&app_state, &mut cache).await?;

        if *shutdown.borrow() {
            break;
        }

        let wakeup = next_wakeup(&app_state).await;

        tokio::select! {
            _ = time::sleep_until(wakeup) => {
                // Scheduled wakeup
            }
            notif = listener.recv() => {
                notif?;
                debug!("bookable write notification received");
            }
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
        };
    }
    Ok(())
}

/// Recompute every bookable payload and broadcast what changed.

async fn publish_bookables(
    app_state: &AppState,
    cache: &mut HashMap<i32, (BookableAssetStatus, BookableAssetQueue)>,
) -> Result<(), anyhow::Error> {
    let statuses = query_as!(
        BookableAssetStatus,
        r#"SELECT
            id as "id!",
            asset_type_id as "asset_type_id!",
            status as "status!: BookableStatus",
            begins,
            ends as "ends: PgDateTime",
            appointment_id
           FROM bookable_asset_status
           ORDER BY id"#
    )
    .fetch_all(&app_state.db)
    .await?;

    let queue_rows = query!(
        r#"SELECT
            asset_id as "asset_id!",
            asset_type_id as "asset_type_id!",
            appointment_id as "appointment_id!",
            begins,
            ends as "ends: PgDateTime",
            room,
            maintenance as "maintenance!",
            bucket as "bucket!"
           FROM bookable_asset_queue
           WHERE (bucket = 'previous' AND past_rank <= $1)
              OR bucket = 'current'
              OR (bucket = 'upcoming' AND future_rank <= $2)
           ORDER BY asset_id, begins"#,
        QUEUE_PREVIOUS_DEPTH,
        QUEUE_UPCOMING_DEPTH,
    )
    .fetch_all(&app_state.db)
    .await?;

    let mut queues: HashMap<i32, BookableAssetQueue> = assemble_queues(
        queue_rows
            .into_iter()
            .map(|r| QueueRow {
                asset_id: r.asset_id,
                asset_type_id: r.asset_type_id,
                appointment_id: r.appointment_id,
                begins: r.begins,
                ends: r.ends,
                room: r.room,
                maintenance: r.maintenance,
                bucket: r.bucket,
            })
            .collect(),
    )
    .into_iter()
    .map(|q| (q.id, q))
    .collect();

    let mut fresh: HashMap<i32, (BookableAssetStatus, BookableAssetQueue)> = HashMap::new();

    for status in statuses {
        // An asset with nothing queued publishes empty buckets to clear the
        // client side.
        let queue = queues.remove(&status.id).unwrap_or(BookableAssetQueue {
            id: status.id,
            asset_type_id: status.asset_type_id,
            previous: Vec::new(),
            current: None,
            upcoming: Vec::new(),
        });

        // Broadcast each channel only when it actually changed, so e.g. a
        // booking made for next week moves the queue without re-sending the
        // status. An asset new to the cache broadcasts both.
        let (changed_status, changed_queue) = match cache.get(&status.id) {
            Some((prev_status, prev_queue)) => (prev_status != &status, prev_queue != &queue),
            None => (true, true),
        };

        if changed_status {
            app_state
                .event_channels
                .bookables
                .broadcast(status.asset_type_id, status.id, status.clone())
                .await;
        }
        if changed_queue {
            app_state
                .event_channels
                .bookable_queues
                .broadcast(queue.asset_type_id, queue.id, queue.clone())
                .await;
        }

        fresh.insert(status.id, (status, queue));
    }

    *cache = fresh;

    Ok(())
}
