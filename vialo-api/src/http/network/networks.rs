use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, grab_authd_conn_user},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::{
    models::{NetAuth, NetworkModel},
    schemas::{NetworkFilterOptions, PostOrPutNetworkSchema},
};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sqlx_conditional_queries::conditional_query_as;

#[utoipa::path(get, path = "/network/networks", params(NetworkFilterOptions), responses((status = 200, description = "OK", body=Vec<NetworkModel>)))]
pub async fn list_networks(
    Query(opts): Query<NetworkFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    let record = conditional_query_as!(
        NetworkModel,
        r#"SELECT id, label, icon, wired, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth as "auth: NetAuth", gen_username, gen_password, active FROM net_networks WHERE TRUE {#wh_search} {#wh_active} LIMIT {limit} OFFSET {offset}"#,
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

#[utoipa::path(put, path = "/network/networks/{id}", request_body = PostOrPutNetworkSchema, responses((status = 204, description = "Updated")))] // no body
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
        wired = $11
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
        body.wired
    )
    .execute(&mut *conn)
    .await?;

    match result.rows_affected() {
        1 => Ok(StatusCode::NO_CONTENT),
        _ => Err(VialoError::NotFound()),
    }
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

#[utoipa::path(post, path = "/network/networks", request_body = PostOrPutNetworkSchema, responses((status = 201, description = "Created")))] // no body
pub async fn post_network(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostOrPutNetworkSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let _result = sqlx::query!(
        "INSERT INTO net_networks (label, icon, multi_device, auto_add_on_auth, auto_add_via_dhcp, auth,gen_username, gen_password, active, wired)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        body.label,
        body.icon,
        body.multi_device,
        body.auto_add_on_auth,
        body.auto_add_via_dhcp,
        body.auth as Option<NetAuth>,
        body.gen_username,
        body.gen_password,
        body.active,
        body.wired
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::CREATED)
}
