use crate::{
    AppState,
    helpers::{PatchOption, encryption},
    http::util::{JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::{models::NmsType, schemas::NetworkFilterOptions};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct NmsConnectorModel {
    pub id: i32,
    #[serde(rename = "type")]
    pub r#type: NmsType,
    pub hostname: String,
    pub site_id: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PostNmsConnectorSchema {
    #[serde(rename = "type")]
    pub r#type: NmsType,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub hostname: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_128")]
    pub site_id: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_512")]
    pub api_key: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub username: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub password: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PutNmsConnectorSchema {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub hostname: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_128")]
    pub site_id: Option<String>,
    pub api_key: PatchOption<String>,
    pub username: PatchOption<String>,
    pub password: PatchOption<String>,
}

#[utoipa::path(get, path = "/network/nms-connectors", params(NetworkFilterOptions), responses((status = 200, description = "OK", body = Vec<NmsConnectorModel>)))]
pub async fn list(
    Query(opts): Query<NetworkFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let records = conditional_query_as!(
        NmsConnectorModel,
        r#"SELECT id, type as "type: NmsType", hostname, site_id FROM net_nms_connectors
           {#search} LIMIT {limit} OFFSET {offset}"#,
        #search = match &opts.search {
            Some(s) => "WHERE hostname ILIKE '%' || {s} || '%'",
            None => "",
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(records)))
}

#[utoipa::path(get, path = "/network/nms-connectors/{id}", responses((status = 200, description = "OK", body = NmsConnectorModel)))]
pub async fn get(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;

    let record = conditional_query_as!(
        NmsConnectorModel,
        r#"SELECT id, type as "type: NmsType", hostname, site_id FROM net_nms_connectors WHERE id = {id}"#,
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/network/nms-connectors", request_body = PostNmsConnectorSchema, responses((status = 201, description = "Created", body = NmsConnectorModel)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostNmsConnectorSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let api_key = body.api_key.as_ref().map(encryption::encrypt).transpose()?;
    let username = body.username.as_ref().map(encryption::encrypt).transpose()?;
    let password = body.password.as_ref().map(encryption::encrypt).transpose()?;

    let record = sqlx::query_as!(
        NmsConnectorModel,
        r#"INSERT INTO net_nms_connectors (type, hostname, site_id, api_key, username, password)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, type as "type: NmsType", hostname, site_id"#,
        body.r#type as NmsType,
        body.hostname,
        body.site_id,
        api_key,
        username,
        password,
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/network/nms-connectors/{id}", request_body = PutNmsConnectorSchema, responses((status = 200, description = "OK", body = NmsConnectorModel)))]
pub async fn put(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<PutNmsConnectorSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let api_key = body.api_key.try_map(|s| encryption::encrypt(&s))?;
    let username = body.username.try_map(|s| encryption::encrypt(&s))?;
    let password = body.password.try_map(|s| encryption::encrypt(&s))?;
    let hostname = body.hostname;
    let site_id = body.site_id;

    let record = conditional_query_as!(
        NmsConnectorModel,
        r#"UPDATE net_nms_connectors SET hostname = {hostname}, site_id = {site_id}
           {#api_key} {#username} {#password}
           WHERE id = {id}
           RETURNING id, type as "type: NmsType", hostname, site_id"#,
        #api_key = match api_key {
            PatchOption::None => "",
            PatchOption::Some(api_key_val) => ", api_key = {api_key_val}",
        },
        #username = match username {
            PatchOption::None => "",
            PatchOption::Some(username_val) => ", username = {username_val}",
        },
        #password = match password {
            PatchOption::None => "",
            PatchOption::Some(password_val) => ", password = {password_val}",
        },
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(delete, path = "/network/nms-connectors/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = query!("DELETE FROM net_nms_connectors WHERE id = $1", id)
        .execute(&mut *conn)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}
