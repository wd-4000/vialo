use crate::{
    AppState,
    helpers::{
        ResponseVariant,
        encryption::{self, Encrypted},
    },
    http::{
        network::{
            devices::models::{
                DeviceWithRefsAndIpAndAccountAndCredentialEmbed, ListDeviceWithRefs,
                NetworkCredentialEmbed,
            },
            mac::MacAddressWrapper,
            models::{CredentialModelWithPassword, NetAuth, NetworkListModel},
        },
        util::{
            JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans, is_unique_violation,
            models::{AccountEmbed, IdOrAllQuery, IdOrMeOrAllQuery, IdOrMeQuery},
        },
    },
    permissions::check_app_role,
};
use rand::{
    distr::{Alphanumeric, SampleString},
    rng,
};
use utoipa::{IntoParams, ToSchema};

use super::models::{DeviceModelWithAccountEmbed, DeviceWithRefs};
use crate::permissions::AppRole;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};

use serde::{Deserialize, Serialize};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct DeviceQuerySchema {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    #[serde(default)]
    pub account_id: IdOrMeOrAllQuery,
}
#[utoipa::path(get, path = "/network/devices", params(DeviceQuerySchema), responses((status = 200, description = "OK", body=ResponseVariant<Vec<DeviceModelWithAccountEmbed>, Vec<ListDeviceWithRefs>>)))]
pub async fn list_devices(
    Query(opts): Query<DeviceQuerySchema>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    // Permission check in case we receive an account ID different from the current account
    let resolved_account_id = opts.account_id.resolve(user.id);
    if resolved_account_id != IdOrAllQuery::Id(user.id) {
        check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    }

    match resolved_account_id {
        IdOrAllQuery::Id(account_id) => {
            let devices = conditional_query_as!(
                ListDeviceWithRefs,
                r#"SELECT net_devices.id, label, hostname,  mac AS "mac: MacAddressWrapper", cred_id, realm_id, net_devices.last_updated,  net_devices.last_seen, net_cred.network_id FROM net_devices
                 JOIN net_cred ON cred_id = net_cred.id WHERE account_id = {account_id} {#wh_search} ORDER by net_devices.id LIMIT {limit} OFFSET {offset}"#,
                #wh_search = match (opts.search){
                    Some(src) => "AND (label ILIKE '%' || {src} || '%' OR mac::text ILIKE '%' || {src} || '%')",
                    _ => ""
                }
        )
        .fetch_all(&data.db)
        .await?;

            Ok(Json(ResponseVariant::B(devices)))
        }
        IdOrAllQuery::All => {
            let devices = conditional_query_as!(DeviceModelWithAccountEmbed,
                r#"SELECT nd.id, nd.label, nd.hostname, mac::macaddr AS "mac?: MacAddressWrapper", cred_id, realm_id, nd.last_updated, nd.last_seen, nc.network_id,
                jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
                FROM net_devices nd
                JOIN net_cred nc ON nd.cred_id = nc.id
                LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id
                {#wh_search} ORDER by nd.id  LIMIT {limit} OFFSET {offset}"#,
                #wh_search = match (opts.search){
                    Some(src) => "WHERE nd.label ILIKE '%' || {src} || '%' OR nd.mac::text ILIKE '%' || {src} || '%'",
                    _ => ""
                }
              ).fetch_all(&data.db)
                .await?;

            Ok(Json(ResponseVariant::A(devices)))
        }
    }
}

#[derive(Deserialize, Debug, ToSchema)]
#[serde(untagged)]
pub enum PostDeviceSchema {
    ForAccount {
        label: String,
        mac: Option<MacAddressWrapper>,
        account_id: IdOrMeQuery,
        network_id: i32,
    },
    ForCred {
        label: String,
        mac: Option<MacAddressWrapper>,
        cred_id: Uuid,
    },
}
#[derive(Serialize, ToSchema)]
pub struct CreatedDeviceResponse {
    #[serde(flatten)]
    pub device: DeviceWithRefs,
    pub credential: CredentialModelWithPassword,
}

#[utoipa::path(post, path = "/network/devices", request_body = PostDeviceSchema, responses((status = 201, description = "Created", body=CreatedDeviceResponse)))]
pub async fn post_device(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostDeviceSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (final_cred, final_label, final_mac) = match body {
        PostDeviceSchema::ForCred {
            cred_id,
            label,
            mac,
        } => {
            let allowed = sqlx::query_scalar!(
                r#"SELECT (
                    (SELECT EXISTS (SELECT 1 FROM net_cred WHERE id = $2 AND account_id = $1))
                    OR
                    account_role_exists($1, 'network_manager'::app_role)
                ) AS "allowed: bool""#,
                user.id,
                cred_id,
            )
            .fetch_one(&mut *trans)
            .await
            .map_err(|e| VialoError::Anyhow(e.into()))?
            .unwrap_or(false);

            if !allowed {
                return Err(VialoError::Forbidden());
            }

            let credential =
                sqlx::query_as!(
                    CredentialModelWithPassword,
                    r#"SELECT id, username, password AS "password: Encrypted<String>", account_id, network_id, last_updated FROM net_cred WHERE id = $1"#,
                    cred_id
                )
                .fetch_one(&mut *trans)
                .await?;

            (credential, label, mac)
        }
        PostDeviceSchema::ForAccount {
            label,
            account_id: account_or_me,
            network_id,
            mac,
        } => {
            let account_id = match account_or_me {
                IdOrMeQuery::Id(id) => {
                    if id != user.id {
                        check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
                    }
                    id
                }
                IdOrMeQuery::Me => user.id,
            };

            let network = sqlx::query_as!(
                NetworkListModel,
                r#"SELECT id, label, icon, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth as "auth: NetAuth", gen_username, gen_password, active, wired, ssid, tls_trust, outer_identity FROM net_networks WHERE id = $1"#,
                network_id
            )
            .fetch_one(&mut *trans)
            .await?;

            // Check if the network is active. If not, require NetworkManager role.
            if !network.active {
                check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
            }

            // Check if we already have a credential we can use
            let mut credential = None;
            if network.multi_device {
                credential = sqlx::query_as!(
                    CredentialModelWithPassword,
                    r#"SELECT id, username, password AS "password: Encrypted<String>", account_id, network_id, last_updated from net_cred WHERE account_id = $1 AND network_id = $2"#,
                    account_id,
                    network_id
                )
                .fetch_optional(&mut *trans)
                .await?;
            }

            // Generate a new credential if not found
            if credential.is_none() {
                let username: String = Alphanumeric.sample_string(&mut rng(), 8);

                let password: String = Alphanumeric.sample_string(&mut rng(), 16);
                let password_encrypted = encryption::encrypt(&password)?;

                credential = Some(
                    sqlx::query_as!(
                        CredentialModelWithPassword,
                        r#"INSERT INTO net_cred (username, password, account_id, network_id, last_updated) VALUES ($1, $2, $3, $4, NOW()) RETURNING id, username, password AS "password: Encrypted<String>", account_id, network_id, last_updated"#,
                        username,
                        password_encrypted,
                        account_id,
                        network_id
                    )
                    .fetch_one(&mut *trans)
                    .await?,
                );
            }

            (credential.unwrap(), label, mac)
        }
    };

    let device = sqlx::query_as!(
              DeviceWithRefs,
              r#"INSERT INTO net_devices (label, mac, cred_id, last_updated) VALUES ($1, $2, $3, NOW()) RETURNING id, label, mac AS "mac: MacAddressWrapper", cred_id, last_updated, last_seen, hostname, realm_id"#,
       final_label, final_mac.map(|a| a.get_value()), final_cred.id)
          .fetch_one(&mut *trans)
          .await
          .map_err(|e| if is_unique_violation(&e) {
              VialoError::AppError(StatusCode::CONFLICT, "mac_conflict".into())
          } else { e.into() })?;

    trans.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedDeviceResponse {
            device,
            credential: final_cred,
        }),
    ))
}

#[utoipa::path(get, path = "/network/devices/{id}/overview", responses((status = 200, description = "OK", body=DeviceWithRefsAndIpAndAccountAndCredentialEmbed)))]
pub async fn get_device_overview(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    return match sqlx::query_as!(
        DeviceWithRefsAndIpAndAccountAndCredentialEmbed,
        r#"SELECT nd.id, nd.label, nd.mac AS "mac: MacAddressWrapper", nd.cred_id, nd.last_updated, nd.last_seen, nd.hostname, nd.realm_id, nia.ipv4_addr as "ipv4_addr?", (row_to_json(nc)::jsonb || jsonb_build_object('network', row_to_json(nn))) as "cred: NetworkCredentialEmbed",
        jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
        FROM net_devices nd
        JOIN net_cred nc ON nd.cred_id = nc.id
        JOIN net_networks nn ON nc.network_id = nn.id
        LEFT JOIN net_ip_assignments nia ON nia.device_id = nd.id
        LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id
        WHERE nc.account_id = $1 AND nd.id = $2"#,
        user.id,
        id
    )
    .fetch_one(&data.db)
    .await
    {
        Ok(device) => Ok(Json(device)),
        Err(_) => {
            // Ok fine, maybe the user's a NetworkManager?
            let is_manager = sqlx::query_scalar!(
                r#"SELECT account_role_exists($1, 'network_manager'::app_role) AS "is_manager: bool""#,
                user.id
            )
            .fetch_one(&data.db)
            .await
            .map_err(|e| VialoError::Anyhow(e.into()))?
            .unwrap_or(false);

            if is_manager {
                return match sqlx::query_as!(
                    DeviceWithRefsAndIpAndAccountAndCredentialEmbed,
                    r#"SELECT nd.id, nd.label, nd.mac AS "mac: MacAddressWrapper", nd.cred_id, nd.last_updated, nd.last_seen, nd.hostname, nd.realm_id, nia.ipv4_addr as "ipv4_addr?", (row_to_json(nc)::jsonb || jsonb_build_object('network', row_to_json(nn))) as "cred: NetworkCredentialEmbed",
                    jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
                    FROM net_devices nd
                    JOIN net_cred nc ON nc.id = nd.cred_id
                    JOIN net_networks nn ON nc.network_id = nn.id
                    LEFT JOIN net_ip_assignments nia ON nia.device_id = nd.id
                    LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id
                    WHERE nd.id = $1"#,
                    id
                )
                .fetch_one(&data.db)
                .await
                {
                    Ok(device) => Ok(Json(device)),
                    Err(e) => {
                        tracing::error!("Failed to fetch device {}: {:?}", id, e);
                        Err(VialoError::NotFound())
                    }
                };
            }

            Err(VialoError::Forbidden())
        }
    };
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct UpdateDeviceSchema {
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub cred_id: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub struct UpdatedDeviceLabelResponse {
    pub label: Option<String>,
}

#[utoipa::path(patch, path = "/network/devices/{id}", request_body = UpdateDeviceSchema, responses((status = 200, description = "Updated", body=UpdatedDeviceLabelResponse)))]
pub async fn update_device(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,

    JsonE(body): JsonE<UpdateDeviceSchema>,
) -> Result<impl IntoResponse, VialoError> {
    if let Err(_) = sqlx::query!(
    "SELECT (
        (SELECT EXISTS (SELECT 1 FROM net_devices JOIN net_cred ON cred_id = net_cred.id WHERE account_id = $1 AND net_devices.id = $2)
            AND (SELECT EXISTS (SELECT 1 FROM net_cred WHERE account_id = $1 AND id = $3 )))
        OR account_role_exists($1, 'network_manager'::app_role)
    ) AS allowed",
    user.id, id, body.cred_id
    )
    .fetch_one(&data.db)
    .await
    {
        return Err(VialoError::Forbidden());
    };

    let device = sqlx::query_as!(
        DeviceWithRefs,
        r#"UPDATE net_devices SET label = coalesce($1, label), mac = coalesce($2, mac), cred_id = coalesce($3, cred_id) WHERE id = $4 RETURNING  id, label, mac AS "mac: MacAddressWrapper", cred_id, last_updated, last_seen, hostname, realm_id"#,
        body.label,
        body.mac.map(|a| a.get_value()),
        body.cred_id,
        id
    )
    .fetch_one(&data.db)
    .await?;

    Ok(Json(UpdatedDeviceLabelResponse {
        label: device.label,
    }))
}

#[utoipa::path(delete, path = "/network/devices/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete_device(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // Must own the device (via credential) OR be a NetworkManager.
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            (SELECT EXISTS (
                SELECT 1 FROM net_devices nd
                JOIN net_cred nc ON nd.cred_id = nc.id
                WHERE nd.id = $2 AND nc.account_id = $1
            ))
            OR account_role_exists($1, 'network_manager'::app_role)
        ) AS "allowed: bool""#,
        user.id,
        id,
    )
    .fetch_one(&data.db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if !allowed {
        return Err(VialoError::Forbidden());
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = sqlx::query!("DELETE FROM net_devices WHERE id = $1", id)
        .execute(&mut *conn)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}
