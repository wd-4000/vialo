use crate::{
    AppState,
    helpers::{
        ResponseVariant,
        encryption::{self, Encrypted},
        random,
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
            JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
            is_unique_violation,
            models::{AccountEmbed, IdOrAllQuery, IdOrMeOrAllQuery, IdOrMeQuery},
        },
    },
    permissions::{AppRole::NetworkManager, check_app_role},
};
use itertools::izip;
use sqlx::{query, query_as};
use utoipa::{IntoParams, ToSchema};

use super::models::{DeviceModelWithAccountEmbed, DeviceWithRefs};
use crate::permissions::AppRole;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;

use serde::{Deserialize, Serialize};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct DeviceQuerySchema {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub network: Option<Vec<i32>>,
    #[serde(default)]
    pub account_id: IdOrMeOrAllQuery,
    pub cred_id: Option<Uuid>,
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
                 JOIN net_cred ON cred_id = net_cred.id WHERE account_id = {account_id} {#wh_network} {#wh_cred} {#wh_search} ORDER by net_devices.id LIMIT {limit} OFFSET {offset}"#,
                #wh_network = match (opts.network) {
                    Some(network) => "AND net_cred.network_id = ANY({network:Vec<i32>})",
                    _ => ""
                },
                 #wh_cred = match (opts.cred_id) {
                     Some(cred_id) => "AND cred_id = {cred_id}",
                     _ => ""
                 },
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
                r#"SELECT nd.id, nd.label, nd.hostname, mac::macaddr AS "mac?: MacAddressWrapper", mobileconfig, cred_id, realm_id, nd.last_updated, nd.last_seen, nc.network_id,
                jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
                FROM net_devices nd
                JOIN net_cred nc ON nd.cred_id = nc.id
                LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id
                WHERE TRUE {#wh_network} {#wh_search} ORDER by nd.id  LIMIT {limit} OFFSET {offset}"#,
                #wh_network = match (opts.network) {
                    Some(network) => "AND nc.network_id = ANY({network:Vec<i32>})",
                    _ => ""
                },
                #wh_search = match (opts.search){
                    Some(src) => "AND (nd.label ILIKE '%' || {src} || '%' OR nd.mac::text ILIKE '%' || {src} || '%')",
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
        #[serde(default)]
        mobileconfig: bool,

        account_id: IdOrMeQuery,
        network_id: i32,
    },
    ForCred {
        label: String,
        mac: Option<MacAddressWrapper>,
        #[serde(default)]
        mobileconfig: bool,

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

    let (final_cred, final_label, final_mac, final_mobileconfig) = match body {
        PostDeviceSchema::ForCred {
            cred_id,
            label,
            mac,
            mobileconfig,
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

            (credential, label, mac, mobileconfig)
        }
        PostDeviceSchema::ForAccount {
            label,
            account_id: account_or_me,
            network_id,
            mac,
            mobileconfig,
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
                r#"SELECT id, label, icon, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth as "auth: NetAuth", gen_username, gen_password, active, migrate_to, wired, ssid, tls_trust, outer_identity FROM net_networks WHERE id = $1"#,
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
                let (username, password) = if let Some(net_auth) = network.auth {
                    let username = if net_auth == NetAuth::UsernamePassword {
                        Some(random::unambiguous_string(
                            network.gen_username.unwrap_or(8) as usize,
                        ))
                    } else {
                        None
                    };

                    (
                        username,
                        Some(random::unambiguous_string(
                            network.gen_password.unwrap_or(16) as usize,
                        )),
                    )
                } else {
                    (None, None)
                };

                let password_encrypted = password.map(|p| encryption::encrypt(&p)).transpose()?;

                credential = Some(
                    sqlx::query_as!(
                        CredentialModelWithPassword,
                        r#"INSERT INTO net_cred (username, password, account_id, network_id, last_updated) VALUES ($1, $2, $3, $4, NOW()) RETURNING id, username, password AS "password: Encrypted<String>", account_id, network_id, last_updated"#,
                        username.as_deref(),
                        password_encrypted,
                        account_id,
                        network_id
                    )
                    .fetch_one(&mut *trans)
                    .await?,
                );
            }

            (credential.unwrap(), label, mac, mobileconfig)
        }
    };

    let device = sqlx::query_as!(
              DeviceWithRefs,
              r#"INSERT INTO net_devices (label, mac, cred_id, mobileconfig, last_updated) VALUES ($1, $2, $3, $4, NOW()) RETURNING id, label, mac AS "mac: MacAddressWrapper", mobileconfig, cred_id, last_updated, last_seen, hostname, realm_id"#,
       final_label, final_mac.map(|a| a.get_value()), final_cred.id, final_mobileconfig)
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
    let query_the = async |user_opt: Option<Uuid>, id: Uuid| -> Result<_, VialoError> {
        let mut device = conditional_query_as!(
            DeviceWithRefsAndIpAndAccountAndCredentialEmbed,
            r#"SELECT nd.id, nd.label, nd.mac AS "mac: MacAddressWrapper", nd.mobileconfig, nd.cred_id, nd.last_updated, nd.last_seen, nd.hostname, nd.realm_id, nia.ipv4_addr as "ipv4_addr?", (row_to_json(nc)::jsonb - 'password' || jsonb_build_object('network', row_to_json(nn))) as "cred: NetworkCredentialEmbed", nc.password as "cred_password: Encrypted<String>",
            jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
            FROM net_devices nd
            JOIN net_cred nc ON nc.id = nd.cred_id
            JOIN net_networks nn ON nc.network_id = nn.id
            LEFT JOIN net_ip_assignments nia ON nia.device_id = nd.id
            {#user}
             nd.id = {id}"#,
            #user = match user_opt {
                Some(user_id) => "JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id WHERE account_id = {user_id} AND",
                None => "LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id WHERE",
            }
        )
        .fetch_one(&data.db)
        .await?;

        // We need to do this because the JSON deserialization facility for NetworkCredentialEmbed does not decrypt it
        if let Some(cred) = device.cred.as_mut() {
            cred.password = device.cred_password.clone();
        }
        return Ok(device);
    };

    // Permission check by way of JOIN and fall-through to the account check
    // for probably zero positive performance impact
    return match query_the(Some(user.id), id).await {
        Ok(device) => Ok(Json(device)),
        Err(_) => {
            // Ok fine, maybe the user's a NetworkManager?
            check_app_role(user, NetworkManager, &data.db).await?;
            return Ok(Json(query_the(None, id).await?));
        }
    };
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct UpdateDeviceSchema {
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub cred_id: Option<Uuid>,
    pub mobileconfig: Option<bool>,
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
        r#"UPDATE net_devices SET label = coalesce($1, label), mac = coalesce($2, mac), cred_id = coalesce($3, cred_id), mobileconfig = coalesce($4, mobileconfig) WHERE id = $5 RETURNING  id, label, mac AS "mac: MacAddressWrapper", mobileconfig, cred_id, last_updated, last_seen, hostname, realm_id"#,
        body.label,
        body.mac.map(|a| a.get_value()),
        body.cred_id,
        body.mobileconfig,
        id
    )
    .fetch_one(&data.db)
    .await?;

    Ok(Json(UpdatedDeviceLabelResponse {
        label: device.label,
    }))
}

/// How multiple device credentials are handled when resetting a password
#[derive(Serialize, Deserialize, Debug, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MultiPasswordHandle {
    ThisDeviceOnly,
    AllDevices,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ResetPasswordSchema {
    pub handle_multi_device: Option<MultiPasswordHandle>,
}

#[derive(Serialize, ToSchema)]
pub struct ResetPasswordResultCredential {
    pub id: Uuid,
    pub username: Option<String>,
    pub password: String,
    pub network_id: i32,
}

#[derive(Serialize, ToSchema)]
pub struct ResetPasswordResultDevice {
    pub id: Uuid,
    pub label: Option<String>,
    pub credential: ResetPasswordResultCredential,
}

#[utoipa::path(post, path = "/network/devices/{id}/reset-password", request_body = ResetPasswordSchema, responses((status = 200, description = "Updated", body=Vec<ResetPasswordResultDevice>)))]
pub async fn reset_password(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,

    JsonE(body): JsonE<ResetPasswordSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Permission
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            (SELECT EXISTS (
                SELECT 1 FROM net_devices nd
                JOIN net_cred nc ON nd.cred_id = nc.id
                WHERE nd.id = $2 AND nc.account_id = $1
            ))
            OR account_role_exists($1, 'network_manager'::app_role)
        ) AS "allowed!: bool""#,
        user.id,
        id,
    )
    .fetch_optional(&data.db)
    .await?
    .unwrap_or(false);

    if !allowed {
        return Err(VialoError::Forbidden());
    }

    // This might be partially the migration network!
    let network = sqlx::query!(
        r#"SELECT coalesce(t.id, n.id) AS "id!", t.auth AS "auth: NetAuth", t.gen_username, t.gen_password,
               n.multi_device, n.active, n.migrate_to, nc.account_id, nd.cred_id, nd.label as device_label, t.multi_device as "t_multi_device"
        FROM net_networks n
        JOIN net_cred nc ON n.id = nc.network_id
        JOIN net_devices nd ON nc.id = nd.cred_id
        JOIN net_networks t ON t.id = coalesce(n.migrate_to, n.id)
        WHERE nd.id = $1"#,
        id
    )
    .fetch_one(&data.db)
    .await?;

    let Some(net_auth) = network.auth else {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "network_no_auth".to_string(),
        ));
    };

    let Some(cred_id) = network.cred_id else {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "device_no_cred".to_string(),
        ));
    };

    let account_id = network.account_id;

    // See if we're using a multi-device credential (actually count).
    // - If network is active, handle according to the handle_multi_device field if needed.
    // - Else, check whether the network has a migration set up.
    // - Else, error out.

    let many_devices = network.multi_device
        && sqlx::query_scalar!(
            r#"SELECT COUNT(*) > 1 AS "many!" FROM net_devices
               WHERE cred_id = $1"#,
            cred_id
        )
        .fetch_one(&data.db)
        .await?;

    #[derive(PartialEq)]
    enum Plan {
        RotateShared,
        SplitThisDevice,
        CredPerDevice,
    }

    let plan = match (
        many_devices,
        body.handle_multi_device,
        network.t_multi_device,
    ) {
        (false, _, _) => Plan::RotateShared,
        (true, Some(MultiPasswordHandle::AllDevices), true) => Plan::RotateShared,
        (true, Some(MultiPasswordHandle::AllDevices), false) => Plan::CredPerDevice,
        (true, Some(MultiPasswordHandle::ThisDeviceOnly), _) => Plan::SplitThisDevice,
        (true, None, _) => {
            return Err(VialoError::AppError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "handle_multi_device is required when many devices are present".to_string(),
            ));
        }
    };

    if !(network.active || network.migrate_to.is_some()) {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "network_inactive".to_string(),
        ));
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?; // begin transaction

    if plan == Plan::RotateShared {
        let new_password = random::unambiguous_string(network.gen_password.unwrap_or(16) as usize);

        let username = match net_auth {
            NetAuth::UsernamePassword => Some(random::unambiguous_string(
                network.gen_username.unwrap_or(8) as usize,
            )),
            NetAuth::Password => None,
        };

        let new_cred = query!(
            "UPDATE net_cred SET
                username = CASE WHEN $2::text IS NULL THEN NULL ELSE coalesce(username, $2) END,
                password = $3, network_id = coalesce($4, network_id), last_updated = NOW()
                WHERE id = $1 RETURNING username",
            cred_id,
            username,
            encryption::encrypt(&new_password)?,
            network.id
        )
        .fetch_one(&mut *trans)
        .await?;

        trans.commit().await?;

        return Ok(Json(vec![ResetPasswordResultDevice {
            id: id,
            label: network.device_label,
            credential: ResetPasswordResultCredential {
                id: cred_id,
                username: new_cred.username,
                password: new_password,
                network_id: network.id,
            },
        }]));
    }

    struct AffectedDevice {
        id: Uuid,
        label: Option<String>,
    }

    let affected_devices = match plan {
        Plan::SplitThisDevice => vec![AffectedDevice {
            id,
            label: network.device_label,
        }],
        _ => {
            // Per-device credentials. Create new credential for every device we touch.
            query_as!(
                AffectedDevice,
                r#"SELECT id, label FROM net_devices WHERE cred_id = $1"#,
                cred_id
            )
            .fetch_all(&mut *trans)
            .await?
        }
    };

    let mut cred_ids = Vec::with_capacity(affected_devices.len());
    let mut usernames = Vec::with_capacity(affected_devices.len());
    let mut passwords = Vec::with_capacity(affected_devices.len());
    let mut passwords_enc = Vec::with_capacity(affected_devices.len());

    for _ in &affected_devices {
        cred_ids.push(Uuid::new_v4());

        usernames.push(match net_auth {
            NetAuth::UsernamePassword => Some(random::unambiguous_string(
                network.gen_username.unwrap_or(8) as usize,
            )),
            NetAuth::Password => None,
        });

        let password = random::unambiguous_string(network.gen_password.unwrap_or(16) as usize);
        passwords_enc.push(encryption::encrypt(&password)?);
        passwords.push(password);
    }

    query!(
        "INSERT INTO net_cred (id, username, password, account_id, network_id, last_updated)
                     SELECT c.id, c.username, c.password, $4, $5, NOW()
                     FROM unnest($1::uuid[], $2::text[], $3::bytea[]) AS c(id, username, password)",
        &cred_ids,
        &usernames as &Vec<Option<String>>,
        &passwords_enc,
        account_id,
        network.id
    )
    .execute(&mut *trans)
    .await?;

    query!(
        "UPDATE net_devices d SET cred_id = p.cred_id, last_updated = NOW()
                     FROM unnest($1::uuid[], $2::uuid[]) AS p(cred_id, device_id)
                     WHERE d.id = p.device_id",
        &cred_ids,
        &affected_devices.iter().map(|d| d.id).collect::<Vec<_>>(),
    )
    .execute(&mut *trans)
    .await?;

    // Delete old cred
    // (the and not exists clause prevents deleting the cred if it's still in use, especially in SplitThisDevice)
    query!(
        "DELETE FROM net_cred WHERE id = $1
           AND NOT EXISTS (SELECT 1 FROM net_devices WHERE cred_id = $1)",
        cred_id,
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    let devices_with_cred: Vec<ResetPasswordResultDevice> =
        izip!(affected_devices, cred_ids, usernames, passwords)
            .map(
                |(device, cred_id, username, password)| ResetPasswordResultDevice {
                    id: device.id,
                    label: device.label,
                    credential: ResetPasswordResultCredential {
                        id: cred_id,
                        username,
                        password,
                        network_id: network.id,
                    },
                },
            )
            .collect();

    Ok(Json(devices_with_cred))
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
