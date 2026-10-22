// pub so the API surface never counts as dead code
pub mod api;
mod commission;
pub mod models;

use super::report::{CommissionFailure, CommissionResult, Discovery};
use super::{Connector, ConnectorRow, OutputStatus, lock_round};
use api::NetioApi;
use models::{Action, OutputPost};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time;

pub(crate) struct Netio;

/// Every linked output's power state on a Netio box
#[derive(Clone, PartialEq)]
pub(crate) struct NetioTarget {
    id: i32,
    api: NetioApi,
    outputs: Vec<NetioOutput>,
}

#[derive(Clone, PartialEq)]
struct NetioOutput {
    /// Output row id, also its lock key
    output: i64,
    post: OutputPost,
}

fn api(connector: &ConnectorRow, proxy: &Option<String>) -> NetioApi {
    NetioApi::new(
        connector.endpoint.clone(),
        connector.username.expose(),
        connector.password.expose(),
        proxy,
    )
}

impl Connector for Netio {
    type Target = NetioTarget;

    async fn discover(
        connector: &ConnectorRow,
        proxy: &Option<String>,
    ) -> Result<Discovery, CommissionFailure> {
        commission::discover(&api(connector, proxy)).await
    }

    async fn commission(
        connector: &ConnectorRow,
        output_id: &str,
        proxy: &Option<String>,
    ) -> CommissionResult {
        commission::check_output(&api(connector, proxy), output_id).await
    }

    fn target(
        connector: &ConnectorRow,
        outputs: Vec<OutputStatus>,
        proxy: &Option<String>,
    ) -> anyhow::Result<NetioTarget> {
        let outputs = outputs
            .into_iter()
            .filter_map(|o| match o.output_id.parse() {
                Ok(id) => Some(NetioOutput {
                    output: o.output,
                    post: OutputPost {
                        id,
                        action: if o.powered { Action::On } else { Action::Off },
                    },
                }),
                Err(_) => {
                    tracing::error!(
                        asset = o.asset_id,
                        output_id = o.output_id,
                        "netio output id isn't a number"
                    );
                    None
                }
            })
            .collect();

        Ok(NetioTarget {
            id: connector.id,
            api: api(connector, proxy),
            outputs,
        })
    }

    async fn sync(mut targets: watch::Receiver<Vec<NetioTarget>>, db: PgPool) {
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

            let current = targets.borrow_and_update().clone();
            let keys = current
                .iter()
                .flat_map(|t| t.outputs.iter().map(|o| o.output))
                .collect();
            let Some((tx, locked)) = lock_round(&db, keys).await else {
                continue;
            };
            let locked = &locked;

            // One request per box, all boxes at once
            futures::future::join_all(current.iter().map(|target| async move {
                let outputs: Vec<OutputPost> = target
                    .outputs
                    .iter()
                    .filter(|o| locked.contains(&o.output))
                    .map(|o| o.post.clone())
                    .collect();
                if outputs.is_empty() {
                    return;
                }
                let res = time::timeout(Duration::from_secs(30), target.api.post(&outputs)).await;
                if let Err(e) = res.map_err(anyhow::Error::from).and_then(|r| r) {
                    tracing::error!(connector = target.id, "netio sync failed: {e:?}");
                }
            }))
            .await;

            // Releases the locks
            if let Err(e) = tx.commit().await {
                tracing::error!("netio sync unlock failed: {e:?}");
            }
        }
    }
}
