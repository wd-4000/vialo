use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, grab_authd_conn_user},
};
use crate::{
    helpers::encryption::{self as encryption, Encrypted},
    printer::{
        self,
        models::{JobData, JobStatus},
    },
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use itertools::{Either, Itertools};
use rand::{
    distr::{Alphanumeric, SampleString},
    rng,
};

use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct PrinterInfo {
    pub id: Uuid,
    pub printer_username: Option<String>,
    #[serde(serialize_with = "encryption::serialize_exposed_opt")]
    #[schema(value_type = Option<String>)]
    pub printer_password: Option<Encrypted<String>>,
    pub printer_id: Option<i32>,
    pub color: Option<i32>,
    pub bw: Option<i32>,
}

#[derive(Serialize, Debug, FromRow)]
pub struct JobWithStatus {
    pub r#type: String,
    pub status: JobStatus,
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct PrinterListModel {
    pub id: Uuid,
    pub printer_username: Option<String>,
    pub printer_id: Option<i32>,
}

#[derive(Deserialize, IntoParams)]
pub struct PrinterFilterOptions {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub account_id_is_null: Option<bool>,
}

#[utoipa::path(get, path = "/people/printer", params(PrinterFilterOptions), responses((status = 200, description = "OK", body=Vec<PrinterListModel>)))]
pub async fn list(
    Query(opts): Query<PrinterFilterOptions>,
    Extension(_user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    // let search = opts.search.unwrap_or("".to_string());
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        PrinterListModel,
        r#"SELECT id as "id!", printer_username, printer_id
        FROM subsystem_printer_context WHERE TRUE {#wh_search} {#wh_account} LIMIT {limit} OFFSET {offset}"#,
            #wh_search = match (opts.search){
                Some(src) => "AND concat(printer_id, printer_username) ILIKE '%' || {src} || '%'",
                _ => ""
            },
            #wh_account = match (opts.account_id_is_null.unwrap_or(false)) {
                true  => "AND id IS NULL",
                false => ""
            }

    )
    .fetch_all(&data.db)
    .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[derive(Serialize, ToSchema)]
pub struct PersonPrinterResponse {
    pub details: Option<PrinterInfo>,
    pub error: Vec<String>,
    pub pending: Vec<String>,
}

#[utoipa::path(get, path = "/people/{id}/printer", responses((status = 200, description = "OK", body=PersonPrinterResponse)))]
pub async fn get(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let details = sqlx::query_as!(
        PrinterInfo,
        r#"SELECT id as "id!", printer_id, printer_username, printer_password as "printer_password: Encrypted<String>", color, bw
        FROM subsystem_printer_context WHERE id = $1 AND id IS NOT NULL"#,
        id
    )
    .fetch_optional(&data.db)
    .await?;

    let (error, pending): (Vec<String>, Vec<String>) = sqlx::query_as!(JobWithStatus,
        r#"SELECT data['type'] as "type!: String", status as "status: JobStatus" FROM subsystem_jobs
        WHERE subsystem = 'printer' AND data->>'account_id' = $1::text AND status != 'done' AND data->>'type' IS NOT NULL"#,
        id.to_string()).fetch_all(&data.db)
    .await?.into_iter().partition_map(|j| {if j.status == JobStatus::Error  {
        Either::Left(j.r#type)
    } else {   Either::Right(j.r#type)}});

    Ok(Json(PersonPrinterResponse {
        details,
        error,
        pending,
    }))
}

#[derive(Deserialize, Debug, Default, ToSchema)]
pub struct LinkOrCreate {
    pub printer_id: Option<i32>,
}

#[utoipa::path(post, path = "/people/{id}/printer", request_body=LinkOrCreate, responses((status = 204, description = "Linked")))]
pub async fn link_or_create(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<LinkOrCreate>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    if let Some(printer_id) = body.printer_id {
        sqlx::query!(
            "UPDATE subsystem_printer_context SET id = $1 WHERE printer_id = $2",
            id,
            printer_id
        )
        .execute(&mut *conn)
        .await?;
    } else {
        let username = Alphanumeric.sample_string(&mut rng(), 8);
        let password = Alphanumeric.sample_string(&mut rng(), 16);

        printer::add_task(
            &mut *conn,
            JobData::CreateAccount {
                account_id: id,
                username: username.clone(),
                password: encryption::encrypt(&password)?,
            },
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/people/{id}/printer", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    sqlx::query!("DELETE FROM subsystem_printer_context WHERE id = $1", id)
        .execute(&mut *conn)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/people/{id}/printer/unlink", responses((status = 204, description = "Unlinked")))]
pub async fn unlink(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    sqlx::query!(
        "UPDATE subsystem_printer_context SET id = NULL WHERE id = $1",
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
