use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::{
    models::{NetAuth, NmsLinkModel, NmsType, NetworkListModel, NetworkModel},
    schemas::{NetworkFilterOptions, PostOrPutNetworkSchema},
};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sqlx_conditional_queries::conditional_query_as;

#[utoipa::path(get, path = "/network/networks", params(NetworkFilterOptions), responses((status = 200, description = "OK", body=Vec<NetworkListModel>)))]
pub async fn list_networks(
    Query(opts): Query<NetworkFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let record = conditional_query_as!(
        NetworkListModel,
        r#"SELECT id, label, icon, wired, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth as "auth: NetAuth", gen_username, gen_password, active, ssid, tls_trust, outer_identity FROM net_networks WHERE TRUE {#wh_search} {#wh_active} LIMIT {limit} OFFSET {offset}"#,
        #wh_search = match (opts.search){
            Some(src) => "AND label ILIKE '%' || {src} || '%'",
            _ => ""
        },
        #wh_active = match (check_app_role(user, AppRole::NetworkManager, &data.db).await.is_ok()){
            true => "",
            false => "AND active = true"
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/network/networks/{id}", responses((status = 200, description = "OK", body = NetworkModel)))]
pub async fn get_network(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;

    let row = sqlx::query!(
        r#"SELECT id, label, icon, wired, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth as "auth: NetAuth", gen_username, gen_password, active, ssid, tls_trust, outer_identity FROM net_networks WHERE id = $1"#,
        id
    )
    .fetch_optional(&data.db)
    .await?
    .ok_or_else(VialoError::NotFound)?;

    let nms_links = sqlx::query_as!(
        NmsLinkModel,
        r#"SELECT nnl.nms_connector_id, nc.type as "type: NmsType", nc.hostname, nnl.nms_id
           FROM net_network_nms_link nnl
           JOIN net_nms_connectors nc ON nc.id = nnl.nms_connector_id
           WHERE nnl.network_id = $1"#,
        id
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(NetworkModel {
        id: row.id,
        label: row.label,
        icon: row.icon,
        wired: row.wired,
        multi_device: row.multi_device,
        auto_add_on_auth: row.auto_add_on_auth,
        auto_add_via_dhcp: row.auto_add_via_dhcp,
        auth: row.auth,
        gen_username: row.gen_username,
        gen_password: row.gen_password,
        active: row.active,
        ssid: row.ssid,
        tls_trust: row.tls_trust,
        outer_identity: row.outer_identity,
        nms_links,
    })))
}

#[utoipa::path(put, path = "/network/networks/{id}", request_body = PostOrPutNetworkSchema, responses((status = 204, description = "Updated")))]
pub async fn put_network(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<PostOrPutNetworkSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let result = sqlx::query!(
        "UPDATE net_networks SET
        label = $2,
        icon = $3,
        multi_device = $4,
        auto_add_on_auth = $5,
        auto_add_via_dhcp = $6,
        auth = $7,
        gen_username = $8,
        gen_password = $9,
        active = $10,
        wired = $11,
        ssid = $12,
        tls_trust = $13,
        outer_identity = $14
        WHERE id = $1",
        id,
        body.label,
        body.icon,
        body.multi_device,
        body.auto_add_on_auth,
        body.auto_add_via_dhcp,
        body.auth as Option<NetAuth>,
        body.gen_username,
        body.gen_password,
        body.active,
        body.wired,
        body.ssid,
        body.tls_trust,
        body.outer_identity,
    )
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() == 0 {
        return Err(VialoError::NotFound());
    }

    if let Some(links) = body.nms_links {
        sqlx::query!(
            "DELETE FROM net_network_nms_link WHERE network_id = $1",
            id
        )
        .execute(&mut *conn)
        .await?;

        for link in links {
            sqlx::query!(
                "INSERT INTO net_network_nms_link (network_id, nms_connector_id, nms_id)
                 VALUES ($1, $2, $3)",
                id,
                link.nms_connector_id,
                link.nms_id,
            )
            .execute(&mut *conn)
            .await?;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/network/networks/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete_network(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = sqlx::query!("DELETE FROM net_networks WHERE id = $1", id)
        .execute(&mut *conn)
        .await
        .unwrap()
        .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/network/networks", request_body = PostOrPutNetworkSchema, responses((status = 201, description = "Created")))]
pub async fn post_network(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostOrPutNetworkSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let network_id = sqlx::query_scalar!(
        "INSERT INTO net_networks (label, icon, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth, gen_username, gen_password, active, wired, ssid, tls_trust, outer_identity)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         RETURNING id",
        body.label,
        body.icon,
        body.multi_device,
        body.auto_add_on_auth,
        body.auto_add_via_dhcp,
        body.auth as Option<NetAuth>,
        body.gen_username,
        body.gen_password,
        body.active,
        body.wired,
        body.ssid,
        body.tls_trust,
        body.outer_identity,
    )
    .fetch_one(&mut *conn)
    .await?;

    if let Some(links) = body.nms_links {
        for link in links {
            sqlx::query!(
                "INSERT INTO net_network_nms_link (network_id, nms_connector_id, nms_id)
                 VALUES ($1, $2, $3)",
                network_id,
                link.nms_connector_id,
                link.nms_id,
            )
            .execute(&mut *conn)
            .await?;
        }
    }

    Ok(StatusCode::CREATED)
}
