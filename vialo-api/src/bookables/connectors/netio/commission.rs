use super::api::NetioApi;
use super::models::NetioGet;
use crate::bookables::connectors::COMMISSION_TIMEOUT;
use crate::bookables::connectors::report::{
    CommissionError, CommissionFailure, CommissionResult, ConnectorInfo, DiscoveredOutput,
    Discovery,
};
use tokio::time;

/// Check the box responds, and list its outputs
pub(super) async fn discover(api: &NetioApi) -> Result<Discovery, CommissionFailure> {
    let res = fetch(api).await?;
    Ok(Discovery {
        info: Some(ConnectorInfo {
            device_name: res.agent.device_name,
            serial_number: res.agent.serial_number,
            mac: res.agent.mac,
        }),
        outputs: res
            .outputs
            .into_iter()
            .map(|o| DiscoveredOutput {
                output_id: o.id.to_string(),
                name: Some(o.name),
            })
            .collect(),
    })
}

/// Check the box responds and still has this output
pub(super) async fn check_output(api: &NetioApi, output_id: &str) -> CommissionResult {
    let res = fetch(api).await?;
    if !res.outputs.iter().any(|o| o.id.to_string() == output_id) {
        return Err(CommissionError::OutputMissing.into());
    }
    Ok(Vec::new())
}

async fn fetch(api: &NetioApi) -> Result<NetioGet, CommissionError> {
    let res = time::timeout(COMMISSION_TIMEOUT, api.get())
        .await
        .map_err(|_| CommissionError::Unreachable {
            detail: "timed out".into(),
        })?;
    Ok(res?)
}
