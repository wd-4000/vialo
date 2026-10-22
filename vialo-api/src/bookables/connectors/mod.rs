//! Connectors control assets' power. It's reaffirmed every minute just to be super sure.
//! For each type, `api` holds the client implementation, while `mod` handles the caching and business logic.

pub mod miele;
pub mod netio;

use super::BookableConnectorType;
use crate::AppState;
use crate::helpers::encryption::Encrypted;
use crate::http::bookables::connectors::BookableConnectorWithPassword;
use sqlx::query_as;
use tokio::sync::watch;

/// An asset's power state on its connector
struct ConnectorAssetStatus {
    id: i32,
    powered: bool,
    connector_output_id: Option<i32>,
}

pub(super) async fn sync_connectors(
    app_state: &AppState,
    miele_targets: &watch::Sender<Vec<miele::MieleConnector>>,
    netio_targets: &watch::Sender<Vec<netio::NetioTarget>>,
) -> Result<(), anyhow::Error> {
    let connectors = query_as!(
        BookableConnectorWithPassword,
        r#"SELECT id,
        type as "type: BookableConnectorType",
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

    let mut miele = Vec::new();
    let mut netio = Vec::new();

    for connector in connectors {
        let statuses = query_as!(
            ConnectorAssetStatus,
            r#"SELECT id as "id!",
            powered as "powered!",
            connector_output_id
        FROM bookable_asset_status
        WHERE connector = $1"#,
            connector.id
        )
        .fetch_all(&app_state.db)
        .await?;

        let id = connector.id;
        match connector.r#type {
            BookableConnectorType::Netio => {
                netio.push(netio::target(connector, statuses, &app_state.config.proxy));
            }
            // Miele connectors are synced via a separate task thing
            BookableConnectorType::Miele => {
                match miele::connector(&connector, statuses, &app_state.config.proxy) {
                    Ok(connector) => miele.push(connector),
                    Err(e) => tracing::error!(connector = id, "bad miele connector: {e:?}"),
                }
            }
        }
    }

    miele_targets.send_if_modified(|current| {
        let changed = *current != miele;
        *current = miele;
        changed
    });
    netio_targets.send_if_modified(|current| {
        let changed = *current != netio;
        *current = netio;
        changed
    });

    Ok(())
}
