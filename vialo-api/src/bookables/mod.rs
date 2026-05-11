use crate::events::EventEnvelope;
use crate::helpers::{PgDateTime, encryption::Encrypted};
use crate::http::bookables::connectors::BookableConnectorWithPassword;
use crate::http::bookables::models::{BookableAssetStatus, BookableStatus};
use crate::{AppState, helpers::grab_authd_conn_subsystem};
use netio::models::OutputPost;
use sqlx::{query, query_as, query_scalar};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time;
use tracing::info;

mod models;

mod netio;
pub use models::*;

pub async fn run(app_state: Arc<AppState>) -> Result<(), anyhow::Error> {
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

        let _outputs: Vec<OutputPost> = statuses
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
        // info!("{:}", json!(NetioPost { outputs }));

        // let res = api
        //     .post()
        //     .json(&NetioPost { outputs })
        //     .send()
        //     .await?
        //     .json::<netio::models::NetioGet>()
        //     .await?;

        // Dry run
        let res = api
            .get()
            .send()
            .await?
            .json::<netio::models::NetioGet>()
            .await?;

        let mut conn = grab_authd_conn_subsystem(&app_state.db, "bookable")
            .await
            .unwrap();

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

async fn next_wakeup(app_state: &AppState) -> tokio::time::Instant {
    let fallback = tokio::time::Instant::now() + Duration::from_secs(3600); // wake up every hour just in case

    let result = query_scalar!(
        r#"SELECT MIN(
            CASE
                WHEN lower(ba.during) > now()::timestamp THEN lower(ba.during)
                ELSE lower(ba.during) + cs.activation_grace_period
            END
        )::timestamptz
        FROM bookable_appointments ba
        LEFT JOIN LATERAL (
            SELECT bs.activation_grace_period
            FROM bookable_schema_assignments bsa
            JOIN bookable_schemas bs ON bs.id = bsa.schema_id
            WHERE bsa.asset_id = ba.asset_id AND bsa.begins <= now()
            ORDER BY bsa.begins DESC
            LIMIT 1
        ) cs ON true
        WHERE ba.cancelled_at IS NULL
        AND ba.activated IS NULL
        AND (
            lower(ba.during) > now()::timestamp
            OR (
                cs.activation_grace_period IS NOT NULL
                AND lower(ba.during) + cs.activation_grace_period > now()::timestamp
            )
        )"#
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
    let (mpsc_tx, mut mpsc_rx) = mpsc::channel::<Arc<EventEnvelope<i32, BookableAssetStatus>>>(10);
    app_state.event_channel.subscribe_wildcard(mpsc_tx).await;

    let mut wakeup = next_wakeup(&app_state).await;

    loop {
        if *shutdown.borrow() {
            break;
        }

        tokio::select! {
            _ = time::sleep_until(wakeup) => {
                // interval sync sort of just in case really
                info!("Bookable Connectors syncing...");
                run(app_state.clone())
                    .await
                    .inspect_err(|e| tracing::error!("{:?}", e))
                    .ok();
                wakeup = next_wakeup(&app_state).await;
            }
            event = mpsc_rx.recv() => {
                // oooh yeah there is an update hell yes !!!
                let Some(event) = event else {
                    break; // channel closed
                };
                info!("event received for {}", event.data.id);
                // TODO be less lazy about this
                run(app_state.clone())
                    .await
                    .inspect_err(|e| tracing::error!("{:?}", e))
                    .ok();
                wakeup = next_wakeup(&app_state).await;
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
