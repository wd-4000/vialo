// pub so the API surface never counts as dead code
pub mod api;
mod models;

use super::ConnectorAssetStatus;
use crate::helpers::grab_authd_conn_subsystem;
use crate::http::bookables::connectors::BookableConnectorWithPassword;
use api::NetioApi;
use models::{Action, Agent, NetioGet, NetioPost, OutputPost};
use sqlx::{PgPool, query};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time;

/// Every asset's power state on a Netio box, and the device info stored for it
#[derive(Clone, PartialEq)]
pub(crate) struct NetioTarget {
    id: i32,
    api: NetioApi,
    outputs: Vec<OutputPost>,
    info: DeviceInfo,
}

/// What the box reports about itself. Just for fun, no harm intended, have a great Thursday everyone
#[derive(Clone, PartialEq)]
struct DeviceInfo {
    num_outputs: Option<i32>,
    device_name: Option<String>,
    serial_number: Option<String>,
    mac: Option<String>,
}

impl From<Agent> for DeviceInfo {
    fn from(agent: Agent) -> Self {
        Self {
            num_outputs: Some(agent.num_outputs),
            device_name: Some(agent.device_name),
            serial_number: Some(agent.serial_number),
            mac: Some(normalize_mac(&agent.mac)),
        }
    }
}

/// Bare lowercase hex, so Postgres' and Netio's spellings compare equal
fn normalize_mac(mac: &str) -> String {
    mac.chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_lowercase()
}

impl NetioTarget {
    /// Push the outputs, returning what the box reports about itself
    async fn push(&mut self) -> anyhow::Result<DeviceInfo> {
        let res = self
            .api
            .post()
            .json(&NetioPost {
                outputs: self.outputs.clone(),
            })
            .send()
            .await?
            .json::<NetioGet>()
            .await?;

        Ok(res.agent.into())
    }
}

/// A connector's target from the asset statuses
pub(super) fn target(
    connector: BookableConnectorWithPassword,
    statuses: Vec<ConnectorAssetStatus>,
    proxy: &Option<String>,
) -> NetioTarget {
    let outputs = statuses
        .into_iter()
        .filter_map(|s| {
            Some(OutputPost {
                id: s.connector_output_id?,
                action: if s.powered { Action::On } else { Action::Off },
            })
        })
        .collect();

    NetioTarget {
        id: connector.id,
        api: NetioApi::new(
            connector.endpoint,
            connector.username.expose(),
            connector.password.expose(),
            proxy,
        ),
        outputs,
        info: DeviceInfo {
            num_outputs: connector.num_outputs,
            device_name: connector.device_name,
            serial_number: connector.serial_number,
            mac: connector.mac.as_deref().map(normalize_mac),
        },
    }
}

/// Push the main loop's outputs every minute or right away when they change
pub(crate) async fn sync(db: PgPool, mut targets: watch::Receiver<Vec<NetioTarget>>) {
    // Connector ID and device info as last stored (we only write upon change)
    let mut stored: HashMap<i32, DeviceInfo> = HashMap::new();
    let mut tick = time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            changed = targets.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }

        let mut current = targets.borrow_and_update().clone();
        stored.retain(|id, _| current.iter().any(|t| t.id == *id));
        for target in &current {
            stored
                .entry(target.id)
                .or_insert_with(|| target.info.clone());
        }

        // One request per box, all boxes at once
        let reported = futures::future::join_all(current.iter_mut().map(|target| async move {
            let res = time::timeout(Duration::from_secs(30), target.push()).await;
            match res.map_err(anyhow::Error::from).and_then(|r| r) {
                Ok(info) => Some((target.id, info)),
                Err(e) => {
                    tracing::error!(connector = target.id, "netio sync failed: {e:?}");
                    None
                }
            }
        }))
        .await;

        for (id, info) in reported.into_iter().flatten() {
            if stored.get(&id) == Some(&info) {
                continue;
            }
            match store_info(&db, id, &info).await {
                Ok(()) => {
                    stored.insert(id, info);
                }
                Err(e) => tracing::error!(connector = id, "netio device info update failed: {e:?}"),
            }
        }
    }
}

async fn store_info(db: &PgPool, id: i32, info: &DeviceInfo) -> anyhow::Result<()> {
    let mut conn = grab_authd_conn_subsystem(db, "bookable")
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    query!(
        "UPDATE bookable_connectors SET num_outputs = $1, device_name = $2, serial_number = $3, mac = $4::text::macaddr WHERE id = $5",
        info.num_outputs,
        info.device_name,
        info.serial_number,
        info.mac,
        id
    ).execute(&mut *conn).await?;

    Ok(())
}
