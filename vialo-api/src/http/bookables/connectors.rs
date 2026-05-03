use super::handlers::BookableFilterOptions;
use crate::AppState;
use crate::helpers::PatchOption;
use crate::helpers::encryption::{self, Encrypted};
use crate::http::util::grab_authd_conn_user;
use crate::http::util::{JsonE, User, VialoError};
use crate::permissions::{AppRole, check_app_role};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct BookableConnector {
    pub id: i32,
    pub endpoint: String,
    pub num_outputs: Option<i32>,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct BookableConnectorWithPassword {
    pub id: i32,
    pub endpoint: String,
    pub num_outputs: Option<i32>,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
    #[schema(value_type = String)]
    pub username: Encrypted<String>,
    #[schema(value_type = String)]
    pub password: Encrypted<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BookableConnectorWithOptionalPassword {
    pub id: i32,
    pub endpoint: String,
    pub num_outputs: Option<i32>,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct BookableConnectorPatch {
    pub endpoint: String,
    pub username: PatchOption<String>,
    pub password: PatchOption<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub struct BookableConnectorPost {
    pub endpoint: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[utoipa::path(get, path = "/bookables/connectors", params(BookableFilterOptions), responses((status = 200, description = "OK", body = Vec<BookableConnector>)))]
pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let limit = opts.limit.unwrap_or(10);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableConnector,
        r#"SELECT id,
            endpoint,
            num_outputs,
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
                endpoint,
                num_outputs,
                device_name,
                serial_number,
                mac::text
            FROM bookable_connectors WHERE id = {id};"#,
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/bookables/connectors", request_body=BookableConnectorPost, responses((status = 201, description = "Created", body=BookableConnector)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(BookableConnectorPost {
        endpoint,
        username,
        password,
    }): JsonE<BookableConnectorPost>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    // Encrypt username and password if provided
    let username = username.as_ref().map(encryption::encrypt).transpose()?;
    let password = password.as_ref().map(encryption::encrypt).transpose()?;

    let record = conditional_query_as!(
        BookableConnector,
        r#"INSERT INTO bookable_connectors (endpoint, username, password)
        VALUES ({endpoint}, {username}, {password})
        RETURNING
            id,
            endpoint,
            num_outputs,
            device_name,
            serial_number,
            mac::text
        ;"#
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/bookables/connectors/{id}", request_body=BookableConnectorPatch, responses((status = 200, description = "Updated", body = BookableConnector)))]
pub async fn put(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(BookableConnectorPatch {
        endpoint,
        username,
        password,
    }): JsonE<BookableConnectorPatch>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let username = username.try_map(|s| encryption::encrypt(&s))?;
    let password = password.try_map(|s| encryption::encrypt(&s))?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let record = conditional_query_as!(BookableConnector,
        r#"UPDATE bookable_connectors SET
        endpoint = {endpoint} {#username} {#password} WHERE id = {id}
        RETURNING
            id,
            endpoint,
            num_outputs,
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
    .fetch_one(&mut *conn)
    .await?;

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
