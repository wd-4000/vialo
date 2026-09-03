use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::helpers::{self, RadiusAttributeValue, grab_authd_conn_subsystem};
use crate::http::util::{JsonE, VialoError};
use crate::{AppState, http::network::mac::MacAddressWrapper};

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RadiusAuthMode {
    Password,
    Tls,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct RadiusAuthRequest {
    #[serde(rename = "Vialo-Auth-Mode")]
    pub mode: RadiusAttributeValue<RadiusAuthMode>,

    #[serde(rename = "User-Name")]
    pub username: Option<RadiusAttributeValue>,

    #[serde(rename = "Calling-Station-Id")]
    pub calling_station_id: Option<RadiusAttributeValue>,

    #[serde(rename = "NAS-IP-Address")]
    pub nas_ip_address: Option<RadiusAttributeValue>,

    #[serde(rename = "TLS-Client-Cert-Serial")]
    pub tls_client_cert_serial: Option<RadiusAttributeValue>,

    #[serde(rename = "TLS-Client-Cert-Issuer")]
    pub tls_client_cert_issuer: Option<RadiusAttributeValue>,

    #[serde(rename = "TLS-Client-Cert-Common-Name")]
    pub tls_client_cert_common_name: Option<RadiusAttributeValue>,

    #[serde(rename = "TLS-Client-Cert-Expiration")]
    pub tls_client_cert_expiration: Option<RadiusAttributeValue>,

    #[serde(rename = "TLS-Client-Cert-X509v3-Extended-Key-Usage")]
    pub tls_client_cert_x509v3_extended_key_usage: Option<RadiusAttributeValue>,
}

/// Error type for the RADIUS hook handlers
#[derive(Debug)]
pub(crate) struct RadiusError {
    status: StatusCode,
    reply_message: String,
}

impl IntoResponse for RadiusError {
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        (
            self.status,
            Json(json!({ "reply_message": self.reply_message })),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for RadiusError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::RowNotFound = err {
            return Self {
                status: StatusCode::NOT_FOUND,
                reply_message: "User not found".to_string(),
            };
        }
        tracing::error!(%err, "radius auth: db error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            reply_message: "Internal error".to_string(),
        }
    }
}

/// Helpers like `grab_authd_conn_subsystem`/`grab_trans` already return `VialoError`
impl From<VialoError> for RadiusError {
    fn from(err: VialoError) -> Self {
        match err {
            VialoError::NotFound() => Self {
                status: StatusCode::NOT_FOUND,
                reply_message: "Not found".to_string(),
            },
            VialoError::Forbidden() => Self {
                status: StatusCode::FORBIDDEN,
                reply_message: "Forbidden".to_string(),
            },
            VialoError::AppError(status, msg) => Self {
                status,
                reply_message: msg,
            },
            other => {
                tracing::error!(?other, "radius auth: internal error");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    reply_message: "Internal error".to_string(),
                }
            }
        }
    }
}

impl From<anyhow::Error> for RadiusError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(%err, "radius auth: internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            reply_message: "Internal error".to_string(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RadiusAuthResponse {
    /// Cleartext password (only for password mode)
    #[serde(
        rename = "control:Cleartext-Password",
        skip_serializing_if = "Option::is_none"
    )]
    password: Option<String>,
    /// cred_id to avoid extra lookup in post-auth
    #[serde(rename = "request:Vialo-Cred-Id")]
    cred_id: Uuid,
    #[serde(rename = "Tunnel-Type")]
    tunnel_type: &'static str,
    #[serde(rename = "Tunnel-Medium-Type")]
    tunnel_medium_type: i32,
    #[serde(rename = "Tunnel-Private-Group-Id")]
    tunnel_private_group_id: i32,
    #[serde(rename = "Mikrotik-Wireless-VLANID")]
    mikrotik_wireless_vlanid: i32,
}

/// RADIUS Authorization: Get a user's password or verify their TLS certificate
#[axum::debug_handler]
#[utoipa::path(
    post,
    path = "/radius/networks/{network_id}/authorize",
    params(
        ("network_id" = i32, Path, description = "Network ID to authenticate against")
    ),
    request_body = RadiusAuthRequest,
    responses(
        (status = 200, description = "Authentication successful, VLAN assigned",
         body = RadiusAuthResponse,
         example = json!({"control:Cleartext-Password": "the_grungler", "VLAN": 200, "Tunnel-Medium-Type": 6, "Tunnel-Private-Group-Id": 100, "Mikrotik-Wireless-VLANID": 100})),
        (status = 404, description = "User or network not found",
         example = json!({"Reply-Message": "User not found"}))
    )
)]
pub(crate) async fn authorize(
    State(data): State<Arc<AppState>>,
    Path(network_id): Path<i32>,
    JsonE(body): JsonE<RadiusAuthRequest>,
) -> Result<impl IntoResponse, RadiusError> {
    let mut conn = grab_authd_conn_subsystem(&data.db, "app").await?;

    match *body.mode {
        RadiusAuthMode::Password => {
            // remove @realm
            let username = body
                .username
                .as_deref()
                .map(|u| u.split('@').next().unwrap_or(u));

            let row = sqlx::query!(
                r#"
                SELECT nc.password,
                       nc.id AS cred_id,
                       COALESCE(nr_device.vlan, nr_user.vlan) AS vlan, -- Handle device VLAN override
                       nt.auto_add_on_auth,
                       nt.multi_device
                FROM net_cred nc
                JOIN net_networks nt ON nc.network_id = nt.id
                JOIN net_realm_assignments nra ON nc.account_id = nra.account_id
                JOIN net_realms nr_user ON nr_user.id = nra.realm_id
                JOIN net_devices nd ON nc.id = nd.cred_id
                LEFT JOIN net_realms nr_device ON nr_device.id = nd.realm_id
                WHERE nc.network_id = $1
                  AND nt.auth = 'username_password'::net_auth
                  AND nc.username = $2
                "#,
                network_id,
                username,
            )
            .fetch_one(&mut *conn)
            .await?;

            let blob = row.password.as_ref().ok_or_else(|| RadiusError {
                status: StatusCode::UNAUTHORIZED,
                reply_message: "No password set".to_string(),
            })?;
            let decrypted_password: String = helpers::encryption::decrypt(blob)?;

            let Some(vlan) = row.vlan else {
                return Err(RadiusError {
                    status: StatusCode::FORBIDDEN,
                    reply_message: "No VLAN".to_string(),
                });
            };

            Ok(Json(RadiusAuthResponse {
                password: Some(decrypted_password),
                cred_id: row.cred_id,
                tunnel_type: "VLAN",
                tunnel_medium_type: 6,
                tunnel_private_group_id: vlan,
                mikrotik_wireless_vlanid: vlan,
            }))
        }
        RadiusAuthMode::Tls => Err(RadiusError {
            status: StatusCode::FORBIDDEN,
            reply_message: "TLS not yet supported".to_string(),
        }),
    }
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct RadiusPostAuthRequest {
    #[serde(rename = "Vialo-Cred-Id")]
    cred_id: Uuid,
    #[serde(rename = "Calling-Station-Id")]
    calling_station_id: MacAddressWrapper,
}

/// RADIUS Post-Auth: Update the credential MAC
#[axum::debug_handler]
#[utoipa::path(
    post,
    path = "/radius/networks/{network_id}/post-auth",
    params(
        ("network_id" = i32, Path, description = "Network ID to authenticate against")
    ),
    request_body = RadiusPostAuthRequest,
    responses(
        (status = 204)
    )
)]
pub(crate) async fn post_auth(
    State(data): State<Arc<AppState>>,
    Path(network_id): Path<i32>,
    JsonE(body): JsonE<RadiusPostAuthRequest>,
) -> Result<impl IntoResponse, RadiusError> {
    let mut conn = grab_authd_conn_subsystem(&data.db, "app").await?;
    let mut trans = crate::http::util::grab_trans(&mut conn).await?;

    let row = sqlx::query!(
        r#"
        SELECT nc.password,
               nc.id AS cred_id,
               nt.auto_add_on_auth,
               nt.multi_device
        FROM net_cred nc
        JOIN net_networks nt ON nc.network_id = nt.id
        JOIN net_realm_assignments nra ON nc.account_id = nra.account_id
        JOIN net_realms nr_user ON nr_user.id = nra.realm_id
        JOIN net_devices nd ON nc.id = nd.cred_id
        LEFT JOIN net_realms nr_device ON nr_device.id = nd.realm_id
        WHERE nc.network_id = $1
          AND nc.id = $2
        "#,
        network_id,
        body.cred_id,
    )
    .fetch_one(&mut *trans)
    .await?;
    // Auto-register the device if the network is configured for it
    if row.auto_add_on_auth {
        let existing_device_id = if !row.multi_device {
            // Singular device per credential — find the one device for this cred
            sqlx::query_scalar!("SELECT id FROM net_devices WHERE cred_id = $1", row.cred_id,)
                .fetch_optional(&mut *trans)
                .await?
        } else {
            // Multi-device — find a free slot (device with no MAC yet)
            sqlx::query_scalar!(
                "SELECT id FROM net_devices WHERE cred_id = $1 AND mac IS NULL",
                row.cred_id,
            )
            .fetch_optional(&mut *trans)
            .await?
        };

        if let Some(device_id) = existing_device_id {
            sqlx::query!(
                "UPDATE net_devices SET (cred_id, mac, hostname) = ($1, $2, NULL) WHERE id = $3",
                row.cred_id,
                body.calling_station_id.get_value(),
                device_id
            )
            .execute(&mut *trans)
            .await?;
        } else {
            sqlx::query!(
                "INSERT INTO net_devices (cred_id, mac) VALUES ($1, $2)
                     ON CONFLICT (mac)
                     DO UPDATE SET (cred_id, mac, hostname) = ($1, $2, NULL)",
                row.cred_id,
                body.calling_station_id.get_value()
            )
            .execute(&mut *trans)
            .await?;
        }
    }

    trans.commit().await?;

    return Ok(StatusCode::NO_CONTENT);
}
