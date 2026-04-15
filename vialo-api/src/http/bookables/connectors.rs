use super::handlers::BookableFilterOptions;
use crate::AppState;
use crate::http::util::grab_authd_conn_user;
use crate::http::util::models::PatchOption;
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
use serde_json::json;
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct BookableConnectorWithPassword {
    pub id: i32,
    pub endpoint: String,
    pub num_outputs: Option<i32>,
    pub device_name: Option<String>,
    pub serial_number: Option<String>,
    pub mac: Option<String>,
    pub username: String,
    pub password: String,
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

#[derive(Deserialize, Debug)]
pub struct BookableConnectorPatch {
    pub endpoint: String,
    pub username: PatchOption<String>,
    pub password: PatchOption<String>,
}

#[derive(Deserialize, Debug)]
pub struct BookableConnectorPost {
    pub endpoint: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let limit = opts.limit.unwrap_or(10);

    let _langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let _offset = (opts.page.unwrap_or(1) - 1) * limit;

    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableConnectorWithOptionalPassword,
        r#"SELECT id,
            endpoint,
            num_outputs,
            device_name,
            serial_number,
            mac::text,
            username,
            password
        FROM bookable_connectors;"#
    )
    .fetch_all(&data.db)
    .await?;

    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

pub async fn get(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let record = conditional_query_as!(
        BookableConnectorWithOptionalPassword,
        r#"SELECT id,
                endpoint,
                num_outputs,
                device_name,
                serial_number,
                mac::text,
                username,
                password
            FROM bookable_connectors WHERE id = {id};"#,
    )
    .fetch_one(&data.db)
    .await?;

    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

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

    let record = conditional_query_as!(
        BookableConnectorWithOptionalPassword,
        r#"INSERT INTO bookable_connectors (endpoint, username, password)
        VALUES ({endpoint}, {username}, {password})
        RETURNING
            id,
            endpoint,
            num_outputs,
            device_name,
            serial_number,
            mac::text,
            username,
            password
        ;"#
    )
    .fetch_one(&mut *conn)
    .await?;

    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

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
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let record = conditional_query_as!(BookableConnectorWithOptionalPassword,
        r#"UPDATE bookable_connectors SET
        endpoint = {endpoint} {#username} {#password} WHERE id = {id}
        RETURNING
            id,
            endpoint,
            num_outputs,
            device_name,
            serial_number,
            mac::text,
            username,
            password
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

    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

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

    return Ok((StatusCode::OK, Json(json!({"status": "success"}))));
}
