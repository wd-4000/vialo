use super::handlers::BookableFilterOptions;
use crate::AppState;
use crate::bookables::BookableConnectorType;
use crate::bookables::connectors::{self, report::CommissionFailure};
use crate::helpers::PatchOption;
use crate::helpers::encryption;
use crate::http::util::{JsonE, User, VialoError, clamp_pagination};
use crate::http::util::{grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_app_role};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{query, query_as};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct CommissionOptions {
    /// Save without commissioning now, leaving it to the bookables module
    #[serde(default)]
    pub skip_commissioning: bool,
}

/// The 422 body when commissioning fails, as built by `require_commissioned`
#[derive(Serialize, ToSchema)]
pub struct CommissioningFailedResponse {
    #[schema(example = "error")]
    status: String,
    #[schema(example = "commissioning_failed")]
    code: String,
    details: CommissionFailure,
}

/// A failed commissioning rolls the request back and goes out as a 422 with the reason
pub(super) fn require_commissioned<T>(res: Result<T, CommissionFailure>) -> Result<T, VialoError> {
    let failure = match res {
        Ok(value) => return Ok(value),
        Err(failure) => failure,
    };
    Err(VialoError::AppErrorWithDetails(
        StatusCode::UNPROCESSABLE_ENTITY,
        "commissioning_failed".into(),
        serde_json::to_value(failure).map_err(anyhow::Error::from)?,
    ))
}

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct BookableConnector {
    pub id: i32,
    pub r#type: BookableConnectorType,
    pub endpoint: String,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BookableConnectorWithOptionalPassword {
    pub id: i32,
    pub r#type: BookableConnectorType,
    pub endpoint: String,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct BookableConnectorPatch {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub endpoint: String,
    pub username: PatchOption<String>,
    pub password: PatchOption<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct BookableConnectorPost {
    pub r#type: BookableConnectorType,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub endpoint: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub username: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub password: String,
}

#[utoipa::path(get, path = "/bookables/connectors", params(BookableFilterOptions), responses((status = 200, description = "OK", body = Vec<BookableConnector>)))]
pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let record = conditional_query_as!(
        BookableConnector,
        r#"SELECT id,
            type as "type: BookableConnectorType",
            endpoint,
            device_name,
            serial_number,
            mac::text
        FROM bookable_connectors {#search} LIMIT {limit} OFFSET {offset}"#,
        #search = match &opts.search {
            Some(s) => "WHERE device_name  ILIKE '%' || {s} || '%'",
            None => "",
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/bookables/connectors/{id}", responses((status = 200, description = "OK", body = BookableConnector)))]
pub async fn get(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let record = conditional_query_as!(
        BookableConnector,
        r#"SELECT id,
                  type as "type: BookableConnectorType",
                  endpoint,
                  device_name,
                  serial_number,
                  mac::text
          FROM bookable_connectors WHERE id = {id};"#,
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

/// One of a connector's outputs, as discovery found it
#[derive(Serialize, Debug, ToSchema)]
pub struct BookableConnectorOutput {
    pub output_id: String,
    pub name: Option<String>,
    /// When discovery last found it. Older than the other outputs': missing from the latest discovery.
    pub last_seen: Option<DateTime<Utc>>,
    pub commissioned_at: Option<DateTime<Utc>>,
    #[schema(value_type = Option<CommissionFailure>)]
    pub commission_error: Option<serde_json::Value>,
    /// Assets linked to it
    pub assets: Vec<i32>,
}

#[utoipa::path(get, path = "/bookables/connectors/{id}/outputs", responses((status = 200, description = "OK", body = Vec<BookableConnectorOutput>)))]
pub async fn outputs(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let record = query_as!(
        BookableConnectorOutput,
        r#"SELECT o.output_id,
            o.name,
            o.last_seen,
            o.commissioned_at,
            o.commission_error,
            ARRAY(SELECT a.id FROM bookable_assets a
                  WHERE a.connector = o.connector AND a.connector_output_id = o.output_id
                  ORDER BY a.id) as "assets!"
        FROM bookable_connector_outputs o
        WHERE o.connector = $1
        ORDER BY o.id"#,
        id
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/bookables/connectors", params(CommissionOptions), request_body=BookableConnectorPost, responses((status = 201, description = "Created", body=BookableConnector), (status = 422, description = "Commissioning failed", body = CommissioningFailedResponse)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Query(opts): Query<CommissionOptions>,
    JsonE(body): JsonE<BookableConnectorPost>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let username = encryption::encrypt(&body.username)?;
    let password = encryption::encrypt(&body.password)?;

    let record = query_as!(
        BookableConnector,
        r#"INSERT INTO bookable_connectors (type, endpoint, username, password)
        VALUES ($1, $2, $3, $4)
        RETURNING
            id,
            type as "type: BookableConnectorType",
            endpoint,
            device_name,
            serial_number,
            mac::text
        ;"#,
        body.r#type as BookableConnectorType,
        body.endpoint,
        username,
        password,
    )
    .fetch_one(&mut *trans)
    .await?;

    if !opts.skip_commissioning {
        let res = connectors::discover(&mut trans, record.id, &data.config.proxy).await?;
        require_commissioned(res)?;
    }

    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/bookables/connectors/{id}", params(CommissionOptions), request_body=BookableConnectorPatch, responses((status = 200, description = "Updated", body = BookableConnector), (status = 422, description = "Commissioning failed", body = CommissioningFailedResponse)))]
pub async fn put(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    Query(opts): Query<CommissionOptions>,
    JsonE(BookableConnectorPatch {
        endpoint,
        username,
        password,
    }): JsonE<BookableConnectorPatch>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let login_changed =
        matches!(username, PatchOption::Some(_)) || matches!(password, PatchOption::Some(_));
    let username = username.try_map(|s| encryption::encrypt(&s))?;
    let password = password.try_map(|s| encryption::encrypt(&s))?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let record = conditional_query_as!(BookableConnector,
        r#"UPDATE bookable_connectors SET
        endpoint = {endpoint}, discovered_at = NULL {#username} {#password} WHERE id = {id}
        RETURNING
            id,
            type as "type: BookableConnectorType",
            endpoint,
            device_name,
            serial_number,
            mac::text
        ;"#,
        #username = match username {
                PatchOption::None => "",
                PatchOption::Some(username) => ", username = {username}"
        },
        #password = match password {
                PatchOption::None => "",
                PatchOption::Some(password) => ", password = {password}"
        }
    )
    .fetch_one(&mut *trans)
    .await?;

    if login_changed {
        connectors::invalidate_commissioning(&mut trans, id).await?;
    }
    if !opts.skip_commissioning {
        let res = connectors::discover(&mut trans, id, &data.config.proxy).await?;
        require_commissioned(res)?;
        // Its linked devices may have moved or lost their login with it
        let res = connectors::commission_linked(&mut trans, id, &data.config.proxy).await?;
        require_commissioned(res)?;
    }

    trans.commit().await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(delete, path = "/bookables/connectors/{id}", responses((status = 204, description = "Deleted")))] //no body
pub async fn delete(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    query!(r#"DELETE FROM bookable_connectors WHERE id = $1;"#, id)
        .execute(&mut *conn)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
