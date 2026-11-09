//! Miele isn't part of this repo, so every operation reports it as not implemented.

use super::report::{CommissionError, CommissionFailure, CommissionResult, Discovery};
use super::{Connector, ConnectorRow, OutputStatus};
use sqlx::PgPool;
use tokio::sync::watch;

const NOT_IMPLEMENTED: &str = "Miele connectors are not implemented";

pub(crate) struct Miele;

#[derive(Clone, PartialEq)]
pub(crate) struct MieleConnector;

fn not_implemented() -> CommissionFailure {
    CommissionError::Other {
        detail: NOT_IMPLEMENTED.into(),
    }
    .into()
}

impl Connector for Miele {
    type Target = MieleConnector;

    async fn discover(
        _connector: &ConnectorRow,
        _proxy: &Option<String>,
    ) -> Result<Discovery, CommissionFailure> {
        Err(not_implemented())
    }

    async fn commission(
        _connector: &ConnectorRow,
        _output_id: &str,
        _proxy: &Option<String>,
    ) -> CommissionResult {
        Err(not_implemented())
    }

    fn target(
        _connector: &ConnectorRow,
        _outputs: Vec<OutputStatus>,
        _proxy: &Option<String>,
    ) -> anyhow::Result<MieleConnector> {
        anyhow::bail!(NOT_IMPLEMENTED)
    }

    /// Never resolves, as a sync task ending means it panicked
    async fn sync(_targets: watch::Receiver<Vec<MieleConnector>>, _db: PgPool) {
        std::future::pending().await
    }
}
