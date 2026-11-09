//! This module covers the Bookables subsystem (so, objects that can be reserved). In our case, that's a room and washing machines.
//! - The bookable [channel] updates are published in this module too.
//!   This is used for WS. The generic channel type is in [crate::events]
//! - Bookable assets that are powered can have a [connector](connectors) assigned.
//!   We run a separate task per connector type that sets the state every minute.
//!
//! main() runs the connector tasks and also handles appointment expiry and notification e-mails.
//! The thread wakes up based on the monstrous query in [next_wakeup].
//!
//! Whenever we need a type both here and in [http](crate::http::bookables), we put it into [models].

mod channel;
pub use channel::{BookableChannel, BookableQueueChannel};

use crate::helpers::PgDateTime;
use crate::http::bookables::models::{
    BookableAssetQueue, BookableAssetStatus, BookableQueueEntry, BookableStatus,
};
use crate::http::util::grab_trans;
use crate::{AppState, helpers::grab_authd_conn_subsystem};
use serde::Deserialize;
use sqlx::{postgres::PgListener, query, query_as, query_scalar};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time;
use tracing::debug;

mod models;

pub mod connectors;
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
            // Maintenance prevails over regular appointment
            "current" if queue.current.as_ref().is_some_and(|c| c.maintenance) => {}
            "current" => queue.current = Some(entry),
            _ => queue.upcoming.push(entry),
        }
    }

    let mut queues: Vec<BookableAssetQueue> = queues.into_values().collect();
    queues.sort_by_key(|q| q.id);
    queues
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
    AND ba.maintenance IS NULL
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
    coalesce(cl.credits - cl.credits_refunded, 0) as refundable_credits,
    ba.cancelled_at,
    ba.cancellation_reason as "cancellation_reason: CancellationReason",
    ba.activated,
    cs.expiry_refund_percent as "expiry_refund_percent!"
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
    WHERE ba.activated IS NULL AND cancelled_at IS NULL AND ba.maintenance IS NULL
        AND cs.requires_activation AND cs.expiry_refund_percent IS NOT NULL
        AND greatest(lower(ba.during), ba.created_at) + cs.activation_grace_period <= NOW()"#,
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
        // Clamp to what an earlier partial refund left behind
        let refund_credits =
            (appointment.credits.unwrap_or(0) * appointment.expiry_refund_percent as i32 / 100)
                .min(appointment.refundable_credits.unwrap_or(0));
        if refund_credits > 0 {
            query!(
                "INSERT INTO credit_ledger (to_account, refund_of, credits, product) VALUES ($1, $2, $3, (SELECT product FROM credit_ledger WHERE id = $2))",
                appointment.from_account,
                appointment.transaction_id,
                refund_credits
            )
            .execute(&mut *trans)
            .await?;
        }
        trans.commit().await?;
    }

    Ok(())
}

/// Cancel appointments under rolling maintenance once its lead is reached, refunding them in full if it says so.
pub async fn cancel_rolling_maintenance_overlaps(
    app_state: &AppState,
) -> Result<(), anyhow::Error> {
    let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    query!(
        r#"WITH due AS (
            -- an appointment can straddle two blocks, refunding wins
            SELECT DISTINCT ON (ba.id) ba.id, ba.transaction_id, m.maintenance = 'refund' AS refund
            FROM bookable_appointments ba
            JOIN bookable_appointments m ON m.asset_id = ba.asset_id AND m.during && ba.during
            WHERE ba.maintenance IS NULL
              AND ba.cancelled_at IS NULL
              AND coalesce(upper(ba.during), 'infinity'::timestamptz) > now()
              AND m.cancelled_at IS NULL
              AND m.maintenance_refund_rolling_lead IS NOT NULL
              AND lower(ba.during) - m.maintenance_refund_rolling_lead <= now()
            ORDER BY ba.id, m.maintenance = 'refund' DESC
        ),
        cancelled AS (
            UPDATE bookable_appointments ba
            SET cancelled_at = now(), cancellation_reason = 'maintenance'
            FROM due
            WHERE ba.id = due.id AND ba.cancelled_at IS NULL
            RETURNING due.transaction_id, due.refund
        )
        INSERT INTO credit_ledger (to_account, refund_of, credits, product)
        SELECT cl.from_account, cl.id, cl.credits - cl.credits_refunded, cl.product
        FROM cancelled c
        JOIN credit_ledger cl ON cl.id = c.transaction_id
        WHERE c.refund AND cl.credits - cl.credits_refunded > 0"#
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Send upcoming reminders. Appointments booked inside their grace period are skipped.
pub async fn remind_upcoming(app_state: &AppState) -> Result<(), anyhow::Error> {
    let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let due = query!(
        r#"UPDATE bookable_appointments ba SET reminded_at = now()
        FROM bookable_notification_prefs p, bookable_assets bast
        WHERE p.account_id = ba.account_id
          AND bast.id = ba.asset_id
          AND 'upcoming' = ANY(p.events)
          AND ba.reminded_at IS NULL
          AND ba.cancelled_at IS NULL
          AND ba.maintenance IS NULL
          AND lower(ba.during) > now()
          AND lower(ba.during) - make_interval(mins => p.upcoming_minutes) <= now()
        RETURNING ba.id,
          ba.account_id as "account_id!",
          lower(ba.during) as "begins!",
          upper(ba.during) as "ends!",
          COALESCE(ba.created_at > lower(ba.during) - make_interval(mins => p.upcoming_minutes), false) as "booked_late!",
          get_i18n_string(bast.name_i18n, $1) as "asset_name?""#,
        &["en".into(), "de".into()],
    )
    .fetch_all(&mut *conn)
    .await?;

    for appointment in due.into_iter().filter(|a| !a.booked_late) {
        if app_state
            .event_channels
            .bookable_events_tx
            .send(BookableEventMessage {
                appointment_id: appointment.id,
                account_id: appointment.account_id,
                asset_name: appointment.asset_name.unwrap_or_else(|| "Unknown".into()),
                begins: appointment.begins,
                ends: appointment.ends,
                event: BookableEvent::Upcoming,
            })
            .is_err()
        {
            tracing::error!(
                appointment_id = %appointment.id,
                "bookable_events channel has no receivers, reminder will not be sent"
            );
        }
    }

    Ok(())
}

#[derive(Deserialize)]
struct BookableEventNotification {
    appointment_id: uuid::Uuid,
    kind: BookableEventKind,
}

/// Turn a `bookable_event` NOTIFY into a `BookableEventMessage`. Runs after commit, so refunds are visible.
async fn publish_bookable_event(app_state: &AppState, payload: &str) -> Result<(), anyhow::Error> {
    let notification: BookableEventNotification = serde_json::from_str(payload)?;

    let Some(appointment) = query!(
        r#"SELECT ba.account_id as "account_id!",
        lower(ba.during) as "begins!",
        upper(ba.during) as "ends!",
        ba.cancellation_reason as "cancellation_reason: CancellationReason",
        cl.credits as "credits?",
        cl.credits_refunded as "credits_refunded?",
        get_i18n_string(bast.name_i18n, $2) as "asset_name?"
        FROM bookable_appointments ba
        JOIN bookable_assets bast ON bast.id = ba.asset_id
        LEFT JOIN credit_ledger cl ON cl.id = ba.transaction_id
        WHERE ba.id = $1"#,
        notification.appointment_id,
        &["en".into(), "de".into()],
    )
    .fetch_optional(&app_state.db)
    .await?
    else {
        return Ok(());
    };

    let credits_refunded = appointment.credits_refunded.unwrap_or(0);
    let event = match notification.kind {
        BookableEventKind::Booked => BookableEvent::Booked {
            credits: appointment.credits.unwrap_or(0),
        },
        BookableEventKind::Canceled => BookableEvent::Canceled {
            reason: appointment.cancellation_reason,
            credits_refunded,
        },
        BookableEventKind::Expired => BookableEvent::Expired { credits_refunded },
        BookableEventKind::Upcoming => {
            return Err(anyhow::anyhow!("upcoming is not a database event"));
        }
    };

    if app_state
        .event_channels
        .bookable_events_tx
        .send(BookableEventMessage {
            appointment_id: notification.appointment_id,
            account_id: appointment.account_id,
            asset_name: appointment.asset_name.unwrap_or_else(|| "Unknown".into()),
            begins: appointment.begins,
            ends: appointment.ends,
            event,
        })
        .is_err()
    {
        tracing::error!(
            appointment_id = %notification.appointment_id,
            "bookable_events channel has no receivers, notification will not be sent"
        );
        crate::health::add_health_event(
            &app_state.db,
            crate::http::history::models::Subsystem::Email,
            "channel_no_receivers",
            Some(serde_json::json!({"appointment_id": notification.appointment_id})),
            50,
            false,
            None,
        )
        .await;
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
        -- next upcoming reminder
        (SELECT MIN(lower(ba.during) - make_interval(mins => p.upcoming_minutes))
           FROM bookable_appointments ba
           JOIN bookable_notification_prefs p ON p.account_id = ba.account_id
          WHERE 'upcoming' = ANY(p.events)
            AND ba.reminded_at IS NULL
            AND ba.cancelled_at IS NULL
            AND ba.maintenance IS NULL
            AND lower(ba.during) > now()),
        -- next rolling maintenance cancellation
        (SELECT MIN(lower(ba.during) - m.maintenance_refund_rolling_lead)
           FROM bookable_appointments ba
           JOIN bookable_appointments m ON m.asset_id = ba.asset_id AND m.during && ba.during
          WHERE m.maintenance_refund_rolling_lead IS NOT NULL
            AND m.cancelled_at IS NULL
            AND ba.maintenance IS NULL
            AND ba.cancelled_at IS NULL
            AND lower(ba.during) - m.maintenance_refund_rolling_lead > now()),
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
    listener
        .listen_all(["bookable_update", "bookable_event", "bookable_reschedule"])
        .await?;

    let mut cache: HashMap<i32, (BookableAssetStatus, BookableAssetQueue)> = HashMap::new();

    let mut connector_tasks = connectors::Connectors::spawn(&app_state.db, &app_state.config.proxy);

    loop {
        cancel_rolling_maintenance_overlaps(&app_state).await?;
        auto_activate_appointments(&app_state).await?;
        expire_appointments(&app_state).await?;
        remind_upcoming(&app_state).await?;
        connector_tasks.sync(&app_state).await?;
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
                let notif = notif?;
                if notif.channel() == "bookable_event" {
                    if let Err(e) = publish_bookable_event(&app_state, notif.payload()).await {
                        tracing::error!("bookable event {} failed: {e:?}", notif.payload());
                    }
                } else {
                    debug!("{} notification received", notif.channel());
                }
            }
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
            res = connector_tasks.stopped() => {
                return Err(anyhow::anyhow!("connector task stopped: {res:?}"));
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
            appointment_id,
            powered as "powered!"
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
    // grouped by asset type so each channel runs its auth filter once per type
    let mut changed_statuses: HashMap<i32, Vec<(i32, BookableAssetStatus)>> = HashMap::new();
    let mut changed_queues: HashMap<i32, Vec<(i32, BookableAssetQueue)>> = HashMap::new();

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
            changed_statuses
                .entry(status.asset_type_id)
                .or_default()
                .push((status.id, status.clone()));
        }
        if changed_queue {
            changed_queues
                .entry(queue.asset_type_id)
                .or_default()
                .push((queue.id, queue.clone()));
        }

        fresh.insert(status.id, (status, queue));
    }

    for (asset_type_id, items) in changed_statuses {
        app_state
            .event_channels
            .bookables
            .broadcast(asset_type_id, items)
            .await;
    }
    for (asset_type_id, items) in changed_queues {
        app_state
            .event_channels
            .bookable_queues
            .broadcast(asset_type_id, items)
            .await;
    }

    *cache = fresh;

    Ok(())
}
