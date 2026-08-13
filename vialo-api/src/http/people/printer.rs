use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans},
    permissions::{AppRole, check_app_role},
};
use crate::{
    helpers::{self},
    printer::models::JobStatus,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use itertools::{Either, Itertools};

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
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::AccountManager, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

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

async fn get_printer_impl(
    target_id: Uuid,
    data: Arc<AppState>,
) -> Result<impl IntoResponse, VialoError> {
    // permission check in the caller!

    let details = sqlx::query_as!(
        PrinterInfo,
        r#"SELECT id as "id!", printer_id, printer_username, color, bw
        FROM subsystem_printer_context WHERE id = $1 AND id IS NOT NULL"#,
        target_id
    )
    .fetch_optional(&data.db)
    .await?;

    let (error, pending): (Vec<String>, Vec<String>) = sqlx::query_as!(JobWithStatus,
        r#"SELECT data['type'] as "type!: String", status as "status: JobStatus" FROM subsystem_jobs
        WHERE subsystem = 'printer' AND data->>'account_id' = $1::text AND status != 'done' AND data->>'type' IS NOT NULL"#,
        target_id.to_string()).fetch_all(&data.db)
    .await?.into_iter().partition_map(|j| {if j.status == JobStatus::Error  {
        Either::Left(j.r#type)
    } else {   Either::Right(j.r#type)}});

    Ok(Json(PersonPrinterResponse {
        details,
        error,
        pending,
    }))
}

#[utoipa::path(get, path = "/people/{id}/printer", responses((status = 200, description = "OK", body=PersonPrinterResponse)))]
pub async fn get_by_id(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id {
        check_app_role(user, AppRole::AccountManager, &data.db).await?;
    }
    get_printer_impl(id, data).await
}

#[utoipa::path(get, path = "/people/me/printer", responses((status = 200, description = "OK", body=PersonPrinterResponse)))]
pub async fn get_me(
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    get_printer_impl(user.id, data).await
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
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
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
        let mut trans = grab_trans(&mut conn).await?;

        let provisioned = sqlx::query_scalar!(
            r#"SELECT (amenities_username IS NOT NULL AND amenities_pin IS NOT NULL) AS "provisioned!"
            FROM accounts_people WHERE id = $1"#,
            id
        )
        .fetch_optional(&mut *trans)
        .await?
        .ok_or(VialoError::NotFound())?;

        if provisioned {
            // Reuse the existing amenities login instead of rotating it.
            // Nothing changed so the sync trigger won't fire, enqueue the sync.
            sqlx::query!(
                "INSERT INTO subsystem_jobs (subsystem, data, last_updated, status)
                 VALUES ('printer', jsonb_build_object('type', 'sync_account', 'account_id', $1::uuid), NOW(), 'pending')",
                id
            )
            .execute(&mut *trans)
            .await?;
        } else {
            helpers::people::generate_amenities_login(id, &mut trans).await?;
        }

        trans.commit().await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/people/{id}/printer", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
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
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    sqlx::query!(
        "UPDATE subsystem_printer_context SET id = NULL WHERE id = $1",
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
