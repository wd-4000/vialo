//! Connectors control assets' power. It's reaffirmed every minute just to be super sure.
//!
//! A Connector is the device we can reach the actual Outputs through.
//!
//! Here's what it looks like for the current state of the code:
//! - Miele: The Miele Gateway is our connector, and the individual Devices are the Outputs.
//!   Usually 1-to-1 in our experience but don't ask why it's this way. Ask why not. Actually don't ask that.
//! - Netio: The box is our Connector, and its individual outputs are the um. Outputs
//!
//! # Commissioning
//! When a new Connector is added, we run through the Commissioning procedure, `commission.rs` in each type's module.
//! 1. Discovery (also when a Connector is edited): we check that the Connector is reachable and store its Outputs in the `bookable_connector_outputs` table.
//!    This lets us have a nice dropdown in the UI.
//! 2. Commissioning (when an asset is linked to an Output): we make sure the Outputs are ready to sync.
//!    In reality this means setting the Admin password and checking permissions for Miele
//!    and checking the Output still exists for Netio.
//!
//! Only commissioned Outputs are synced, never while they're being commissioned. Skipped or failed ones are retried
//! in the background, except rejected logins, which would only get Miele devices to block the login.
//!
//! For each type, `api` holds the client implementation, `commission` the setup, and `mod` the sync.

pub mod error;
pub mod miele;
pub mod netio;
pub mod report;

use super::BookableConnectorType;
use crate::AppState;
use crate::helpers::{encryption::Encrypted, grab_authd_conn_subsystem};
use report::{CommissionFailure, CommissionResult, Discovery};
use reqwest::Url;
use sqlx::{Connection, PgPool, Postgres, Transaction, query, query_as, query_scalar};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time;

/// Per device timeout while commissioning
const COMMISSION_TIMEOUT: Duration = Duration::from_secs(20);

/// How often pending discovery and commissioning are retried in the background
const COMMISSION_RETRY: Duration = Duration::from_secs(300);

/// A connector with its credentials
pub(crate) struct ConnectorRow {
    id: i32,
    r#type: BookableConnectorType,
    endpoint: String,
    username: Encrypted<String>,
    password: Encrypted<String>,
}

/// A linked asset's power state and the output it's on
pub(crate) struct OutputStatus {
    asset_id: i32,
    powered: bool,
    /// `bookable_connector_outputs` row, also the output's advisory lock key
    output: i32,
    /// The output ID in the connector's terms
    output_id: String,
}

pub(crate) trait Connector {
    type Target: Clone + PartialEq + Send + Sync + 'static;

    /// Check the connector responds and list its outputs
    fn discover(
        connector: &ConnectorRow,
        proxy: &Option<String>,
    ) -> impl Future<Output = Result<Discovery, CommissionFailure>> + Send;

    /// Get one Output (Miele = Device) ready for sync. Safe to re-run.
    fn commission(
        connector: &ConnectorRow,
        output_id: &str,
        proxy: &Option<String>,
    ) -> impl Future<Output = CommissionResult> + Send;

    /// What the sync task should apply for this connector
    fn target(
        connector: &ConnectorRow,
        outputs: Vec<OutputStatus>,
        proxy: &Option<String>,
    ) -> anyhow::Result<Self::Target>;

    /// Apply the targets every minute or right away when they change, skipping outputs being commissioned
    fn sync(
        targets: watch::Receiver<Vec<Self::Target>>,
        db: PgPool,
    ) -> impl Future<Output = ()> + Send;
}

/// `url` with `segments` appended, each escaped as exactly one path segment
fn with_segments(url: &Url, segments: &[&str]) -> anyhow::Result<Url> {
    let mut url = url.clone();
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("can't be a base URL"))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

async fn fetch_connectors(
    db: impl sqlx::PgExecutor<'_>,
    id: Option<i32>,
) -> sqlx::Result<Vec<ConnectorRow>> {
    query_as!(
        ConnectorRow,
        r#"SELECT id,
        type as "type: BookableConnectorType",
        endpoint,
        username as "username!: Encrypted<String>",
        password as "password!: Encrypted<String>"
    FROM bookable_connectors
    WHERE $1::int IS NULL OR id = $1"#,
        id
    )
    .fetch_all(db)
    .await
}

/// The connector background tasks for running syncs and commissioning whatever is still pending
pub(super) struct Connectors {
    miele: watch::Sender<Vec<miele::MieleConnector>>,
    netio: watch::Sender<Vec<netio::NetioTarget>>,
    tasks: JoinSet<()>,
}

impl Connectors {
    pub(super) fn spawn(db: &PgPool, proxy: &Option<String>) -> Self {
        let (miele, miele_rx) = watch::channel(Vec::new());
        let (netio, netio_rx) = watch::channel(Vec::new());

        let mut tasks = JoinSet::new();
        tasks.spawn(miele::Miele::sync(miele_rx, db.clone()));
        tasks.spawn(netio::Netio::sync(netio_rx, db.clone()));
        tasks.spawn(commission_pending(db.clone(), proxy.clone()));

        Self {
            miele,
            netio,
            tasks,
        }
    }

    /// Rebuild every connector's targets from its commissioned outputs and hand them to the syncs
    pub(super) async fn sync(&self, app_state: &AppState) -> anyhow::Result<()> {
        let proxy = &app_state.config.proxy;
        let mut miele = Vec::new();
        let mut netio = Vec::new();

        for connector in fetch_connectors(&app_state.db, None).await? {
            let outputs = query_as!(
                OutputStatus,
                r#"SELECT s.id as "asset_id!",
                s.powered as "powered!",
                o.id as "output!",
                o.output_id as "output_id!"
            FROM bookable_asset_status s
            JOIN bookable_connector_outputs o
              ON o.connector = s.connector AND o.output_id = s.connector_output_id
            WHERE s.connector = $1 AND o.commissioned_at IS NOT NULL"#,
                connector.id
            )
            .fetch_all(&app_state.db)
            .await?;

            let res = match connector.r#type {
                BookableConnectorType::Netio => {
                    netio::Netio::target(&connector, outputs, proxy).map(|t| netio.push(t))
                }
                BookableConnectorType::Miele => {
                    miele::Miele::target(&connector, outputs, proxy).map(|t| miele.push(t))
                }
            };
            if let Err(e) = res {
                tracing::error!(connector = connector.id, "bad connector: {e:?}");
            }
        }

        publish(&self.miele, miele);
        publish(&self.netio, netio);

        Ok(())
    }

    /// Resolves once a task ends, which they only do by panicking
    pub(super) async fn stopped(&mut self) -> Option<Result<(), JoinError>> {
        self.tasks.join_next().await
    }
}

fn publish<T: PartialEq>(tx: &watch::Sender<Vec<T>>, targets: Vec<T>) {
    tx.send_if_modified(|current| {
        let changed = *current != targets;
        *current = targets;
        changed
    });
}

/// Start a sync round: lock what we can of `keys` until the transaction ends. Commissioning holds the rest.
async fn lock_round(
    db: &PgPool,
    keys: Vec<i32>,
) -> Option<(Transaction<'static, Postgres>, HashSet<i32>)> {
    let res = async {
        let mut tx = db.begin().await?;
        let locked = query_scalar!(
            r#"SELECT key as "key!" FROM unnest($1::int[]) key WHERE pg_try_advisory_xact_lock(key)"#,
            &keys
        )
        .fetch_all(&mut *tx)
        .await?;
        anyhow::Ok((tx, locked.into_iter().collect()))
    }
    .await;

    res.inspect_err(|e| tracing::error!("connector sync lock failed: {e:?}"))
        .ok()
}

/// Connector level commissioning, storing the outcome and, when it worked, what it found. Committing is up to the caller.
pub(crate) async fn discover(
    tx: &mut Transaction<'_, Postgres>,
    id: i32,
    proxy: &Option<String>,
) -> anyhow::Result<Result<(), CommissionFailure>> {
    let Some(connector) = fetch_connectors(&mut **tx, Some(id)).await?.pop() else {
        anyhow::bail!("no connector {id}");
    };

    let res = match connector.r#type {
        BookableConnectorType::Netio => netio::Netio::discover(&connector, proxy).await,
        BookableConnectorType::Miele => miele::Miele::discover(&connector, proxy).await,
    };

    query!(
        "UPDATE bookable_connectors SET discovered_at = CASE WHEN $1 THEN now() END, discovery_error = $2 WHERE id = $3",
        res.is_ok(),
        res.as_ref().err().map(serde_json::to_value).transpose()?,
        id
    )
    .execute(&mut **tx)
    .await?;
    let discovery = match res {
        Ok(discovery) => discovery,
        Err(failure) => return Ok(Err(failure)),
    };

    if let Some(info) = &discovery.info {
        query!(
            "UPDATE bookable_connectors SET device_name = $1, serial_number = $2, mac = $3::text::macaddr WHERE id = $4",
            info.device_name,
            info.serial_number,
            info.mac,
            id
        )
        .execute(&mut **tx)
        .await?;
    }

    let (output_ids, names): (Vec<String>, Vec<Option<String>>) = discovery
        .outputs
        .into_iter()
        .map(|o| (o.output_id, o.name))
        .unzip();
    query!(
        "INSERT INTO bookable_connector_outputs (connector, output_id, name, last_seen)
        SELECT $1, output_id, name, now() FROM unnest($2::text[], $3::text[]) AS o(output_id, name)
        ON CONFLICT (connector, output_id) DO UPDATE SET name = EXCLUDED.name, last_seen = EXCLUDED.last_seen",
        id,
        &output_ids,
        &names as &[Option<String>],
    )
    .execute(&mut **tx)
    .await?;

    Ok(Ok(()))
}

/// Output level commissioning under the output's lock, storing the outcome. Committing is up to the caller.
pub(crate) async fn commission_output(
    tx: &mut Transaction<'_, Postgres>,
    connector: i32,
    output_id: &str,
    proxy: &Option<String>,
) -> anyhow::Result<CommissionResult> {
    let output = query_scalar!(
        "SELECT id FROM bookable_connector_outputs WHERE connector = $1 AND output_id = $2",
        connector,
        output_id
    )
    .fetch_one(&mut **tx)
    .await?;

    lock_output(tx, output).await?;
    commission_locked(tx, output, connector, output_id, proxy).await
}

/// Commission every output an asset links to on this connector, e.g. after its login changed.
/// Stops at the first failure, the caller rolls back anyway.
pub(crate) async fn commission_linked(
    tx: &mut Transaction<'_, Postgres>,
    connector: i32,
    proxy: &Option<String>,
) -> anyhow::Result<Result<(), CommissionFailure>> {
    let output_ids = query_scalar!(
        r#"SELECT DISTINCT connector_output_id as "output_id!" FROM bookable_assets
        WHERE connector = $1 AND connector_output_id IS NOT NULL ORDER BY 1"#,
        connector
    )
    .fetch_all(&mut **tx)
    .await?;

    for output_id in output_ids {
        if let Err(failure) = commission_output(tx, connector, &output_id, proxy).await? {
            return Ok(Err(failure));
        }
    }
    Ok(Ok(()))
}

/// Wait out a sync round touching the output, but not forever. Held until the transaction ends.
async fn lock_output(tx: &mut Transaction<'_, Postgres>, output: i32) -> sqlx::Result<()> {
    query_scalar!("SELECT set_config('lock_timeout', '45s', true)")
        .fetch_one(&mut **tx)
        .await?;
    query_scalar!("SELECT 1 FROM pg_advisory_xact_lock($1::int)", output)
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

async fn commission_locked(
    tx: &mut Transaction<'_, Postgres>,
    output: i32,
    connector: i32,
    output_id: &str,
    proxy: &Option<String>,
) -> anyhow::Result<CommissionResult> {
    let Some(row) = fetch_connectors(&mut **tx, Some(connector)).await?.pop() else {
        anyhow::bail!("no connector {connector}");
    };

    let res = match row.r#type {
        BookableConnectorType::Netio => netio::Netio::commission(&row, output_id, proxy).await,
        BookableConnectorType::Miele => miele::Miele::commission(&row, output_id, proxy).await,
    };
    let res = res.map_err(|failure| CommissionFailure {
        output: Some(output_id.into()),
        ..failure
    });

    query!(
        "UPDATE bookable_connector_outputs SET commissioned_at = CASE WHEN $1 THEN now() END, commission_error = $2 WHERE id = $3",
        res.is_ok(),
        res.as_ref().err().map(serde_json::to_value).transpose()?,
        output
    )
    .execute(&mut **tx)
    .await?;

    Ok(res)
}

/// Discover pending connectors and commission pending linked outputs, every few minutes
async fn commission_pending(db: PgPool, proxy: Option<String>) {
    let mut tick = time::interval(COMMISSION_RETRY);
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;
        if let Err(e) = commission_round(&db, &proxy).await {
            tracing::error!("background commissioning failed: {e:?}");
        }
    }
}

/// Discover undiscovered/errored connector outputs, and commission every output
async fn commission_round(db: &PgPool, proxy: &Option<String>) -> anyhow::Result<()> {
    let mut conn = grab_authd_conn_subsystem(db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // Discovery first, it may make linked outputs commissionable this round
    let connectors =
        query!("SELECT id, discovery_error FROM bookable_connectors WHERE discovered_at IS NULL")
            .fetch_all(&mut *conn)
            .await?;
    for c in connectors {
        let connector = c.id;
        let res = async {
            let mut tx = conn.begin().await?;
            let res = discover(&mut tx, connector, proxy).await?;
            tx.commit().await?;
            anyhow::Ok(res)
        }
        .await;
        match res {
            Ok(Ok(())) => tracing::info!(connector, "discovered in the background"),
            Ok(Err(f)) if f.is_new(c.discovery_error.as_ref()) => {
                tracing::warn!(connector, "background discovery failed: {f:?}")
            }
            Ok(Err(f)) => {
                tracing::debug!(connector, "background discovery still failing: {f:?}")
            }
            Err(e) => tracing::error!(connector, "background discovery errored: {e:?}"),
        }
    }

    // Rejected logins wait for the login to change or a manual retry
    let outputs = query!(
        "SELECT o.id, o.connector, o.output_id, o.commission_error
        FROM bookable_connector_outputs o
        WHERE o.commissioned_at IS NULL
          AND coalesce(o.commission_error->>'code', '') NOT IN ('login_rejected', 'login_blocked')
          AND EXISTS (SELECT 1 FROM bookable_assets a
                      WHERE a.connector = o.connector AND a.connector_output_id = o.output_id)"
    )
    .fetch_all(&mut *conn)
    .await?;
    for o in outputs {
        let res = async {
            let mut tx = conn.begin().await?;
            lock_output(&mut tx, o.id).await?;
            // A request may have commissioned it while we waited
            let pending = query_scalar!(
                r#"SELECT commissioned_at IS NULL as "pending!" FROM bookable_connector_outputs WHERE id = $1"#,
                o.id
            )
            .fetch_one(&mut *tx)
            .await?;
            if !pending {
                return anyhow::Ok(None);
            }
            let res = commission_locked(&mut tx, o.id, o.connector, &o.output_id, proxy).await?;
            tx.commit().await?;
            anyhow::Ok(Some(res))
        }
        .await
        .transpose();
        let (connector, output) = (o.connector, &o.output_id);
        match res {
            None => {}
            Some(Ok(Ok(_))) => {
                tracing::info!(connector, output, "commissioned in the background")
            }
            Some(Ok(Err(f))) if f.is_new(o.commission_error.as_ref()) => {
                tracing::warn!(connector, output, "background commissioning failed: {f:?}")
            }
            Some(Ok(Err(f))) => {
                tracing::debug!(
                    connector,
                    output,
                    "background commissioning still failing: {f:?}"
                )
            }
            Some(Err(e)) => {
                tracing::error!(connector, output, "background commissioning errored: {e:?}")
            }
        }
    }

    Ok(())
}

/// The connector's login changed, so its devices need commissioning again
pub(crate) async fn invalidate_commissioning(
    tx: &mut Transaction<'_, Postgres>,
    connector: i32,
) -> sqlx::Result<()> {
    query!(
        "UPDATE bookable_connector_outputs SET commissioned_at = NULL, commission_error = NULL WHERE connector = $1",
        connector
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
