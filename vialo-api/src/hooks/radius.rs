use std::str::FromStr;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::helpers::{self, grab_authd_conn_subsystem};
use crate::http::util::{JsonE, VialoError};

/// A single RADIUS attribute value as sent by FreeRADIUS rlm_rest.
/// Each attribute is an object with a `type` field (e.g. "string", "ipaddr")
/// and a `value` array (always a JSON array, even for scalar attributes).
#[derive(Deserialize, Debug, ToSchema)]
pub struct RadiusAttributeValue {
    #[serde(rename = "type")]
    pub attr_type: String,
    pub value: Vec<String>,
}

/// FreeRADIUS rlm_rest request body
#[derive(Deserialize, Debug, ToSchema)]
pub struct RadiusAuthBody {
    #[serde(rename = "User-Name")]
    pub user_name: Option<RadiusAttributeValue>,
    #[serde(rename = "User-Password")]
    pub user_password: Option<RadiusAttributeValue>,
    #[serde(rename = "Calling-Station-Id")]
    pub calling_station_id: Option<RadiusAttributeValue>,
    #[serde(rename = "NAS-IP-Address")]
    pub nas_ip_address: Option<RadiusAttributeValue>,
}

/// Serializes directly to the FreeRADIUS `rlm_rest` JSON wire format.
/// Not exposed as an OpenAPI body — the response shape is protocol-defined.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum RadiusResponse {
    Accept {
        #[serde(rename = "Tunnel-Type")]
        tunnel_type: &'static str,
        #[serde(rename = "Tunnel-Medium-Type")]
        tunnel_medium_type: i32,
        #[serde(rename = "Tunnel-Private-Group-Id")]
        tunnel_private_group_id: i32,
    },
    Reject {
        #[serde(rename = "Reply-Message")]
        reply_message: String,
        #[serde(skip)]
        status: StatusCode,
    },
}

impl IntoResponse for RadiusResponse {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            RadiusResponse::Accept { .. } => StatusCode::OK,
            RadiusResponse::Reject { status, .. } => *status,
        };
        (status, Json(self)).into_response()
    }
}

/// Authenticate a RADIUS access request, adding the device if `auto_add_on_auth` is enabled on the network.
#[utoipa::path(
    post,
    path = "/radius/auth/{network_id}",
    params(
        ("network_id" = i32, Path, description = "Network ID to authenticate against")
    ),
    request_body = RadiusAuthBody,
    responses(
        (status = 200, description = "Authentication successful — VLAN assigned",
         example = json!({"Tunnel-Type": "VLAN", "Tunnel-Medium-Type": 6, "Tunnel-Private-Group-Id": 100})),
        (status = 401, description = "Invalid password",
         example = json!({"Reply-Message": "Invalid password"})),
        (status = 404, description = "User or network not found",
         example = json!({"Reply-Message": "User not found"}))
    )
)]
pub(crate) async fn auth(
    State(data): State<Arc<AppState>>,
    Path(network_id): Path<i32>,
    JsonE(body): JsonE<RadiusAuthBody>,
) -> Result<RadiusResponse, VialoError> {
    let username = body
        .user_name
        .as_ref()
        .and_then(|a| a.value.first())
        .ok_or_else(|| VialoError::AppError(StatusCode::BAD_REQUEST, "Missing User-Name".into()))?;

    let password = body
        .user_password
        .as_ref()
        .and_then(|a| a.value.first())
        .ok_or_else(|| {
            VialoError::AppError(StatusCode::BAD_REQUEST, "Missing User-Password".into())
        })?;

    let mac_str = body
        .calling_station_id
        .as_ref()
        .and_then(|a| a.value.first())
        .map(|s| s.as_str());

    let mac = mac_str.and_then(|s| MacAddress::from_str(s).ok());

    let mut conn = grab_authd_conn_subsystem(&data.db, "app").await?;
    let mut trans = crate::http::util::grab_trans(&mut conn).await?;

    // Query matching credentials by (network_id, username)
    let rows = sqlx::query!(
        r#"
        SELECT nc.password,
               nc.id AS cred_id,
               nr.vlan,
               nt.auto_add_on_auth,
               nt.multi_device
        FROM net_cred nc
        JOIN net_networks nt ON nc.network_id = nt.id
        JOIN net_realm_assignments nra ON nc.account_id = nra.account_id
        JOIN net_realms nr ON nr.id = nra.realm_id
        WHERE nc.network_id = $1
          AND nt.auth = 'username_password'::net_auth
          AND nc.username = $2
        "#,
        network_id,
        username,
    )
    .fetch_all(&mut *trans)
    .await?;

    // Decrypt and compare each credential's password
    struct CredMatch {
        vlan: Option<i32>,
        cred_id: Uuid,
        auto_add_on_auth: bool,
        multi_device: bool,
    }

    let matched = rows.iter().find_map(|row| {
        let blob = row.password.as_ref()?;
        let decrypted: String = helpers::encryption::decrypt(blob).ok()?;
        if decrypted == *password {
            Some(CredMatch {
                vlan: row.vlan,
                cred_id: row.cred_id,
                auto_add_on_auth: row.auto_add_on_auth,
                multi_device: row.multi_device,
            })
        } else {
            None
        }
    });

    let m = match matched {
        Some(m) => m,
        None if rows.is_empty() => {
            return Ok(RadiusResponse::Reject {
                status: StatusCode::NOT_FOUND,
                reply_message: "User not found".into(),
            });
        }
        None => {
            return Ok(RadiusResponse::Reject {
                status: StatusCode::UNAUTHORIZED,
                reply_message: "Invalid password".into(),
            });
        }
    };

    // Check for a device-level VLAN override (net_devices.realm_id)
    let vlan = if let Some(mac_addr) = mac {
        let override_vlan = sqlx::query_scalar!(
            r#"
            SELECT nr.vlan
            FROM net_devices nd
            JOIN net_realms nr ON nd.realm_id = nr.id
            WHERE nd.mac = $1
            "#,
            mac_addr as MacAddress,
        )
        .fetch_optional(&mut *trans)
        .await?;

        override_vlan.flatten().or(m.vlan)
    } else {
        m.vlan
    };

    // Auto-register the device if the network is configured for it
    if m.auto_add_on_auth
        && let Some(mac_addr) = mac {
            let existing_device_id = if !m.multi_device {
                // Singular device per credential — find the one device for this cred
                sqlx::query_scalar!("SELECT id FROM net_devices WHERE cred_id = $1", m.cred_id,)
                    .fetch_optional(&mut *trans)
                    .await?
            } else {
                // Multi-device — find a free slot (device with no MAC yet)
                sqlx::query_scalar!(
                    "SELECT id FROM net_devices WHERE cred_id = $1 AND mac IS NULL",
                    m.cred_id,
                )
                .fetch_optional(&mut *trans)
                .await?
            };

            if let Some(device_id) = existing_device_id {
                sqlx::query!(
                    "UPDATE net_devices SET (cred_id, mac, hostname) = ($1, $2, NULL) WHERE id = $3",
                    m.cred_id,
                    mac_addr as MacAddress,
                    device_id,
                )
                .execute(&mut *trans)
                .await?;
            } else {
                sqlx::query!(
                    "INSERT INTO net_devices (cred_id, mac) VALUES ($1, $2)
                     ON CONFLICT (mac)
                     DO UPDATE SET (cred_id, mac, hostname) = ($1, $2, NULL)",
                    m.cred_id,
                    mac_addr as MacAddress,
                )
                .execute(&mut *trans)
                .await?;
            }
        }

    trans.commit().await?;

    Ok(RadiusResponse::Accept {
        tunnel_type: "VLAN",
        tunnel_medium_type: 6,
        tunnel_private_group_id: vlan.unwrap_or(0),
    })
}
