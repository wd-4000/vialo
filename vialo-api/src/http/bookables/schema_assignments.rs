use super::permissions::{BookablePerm, require_asset_type_perm_by_asset};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::{
    AppState,
    http::util::{JsonE, ListOptions, User, VialoError, clamp_pagination, grab_authd_conn_user},
};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct BookableSchemaAssignment {
    pub begins: DateTime<Utc>,
    pub schema_id: i32,
}

#[utoipa::path(get, path = "/bookables/{id}/schema_assignments", params(ListOptions), responses((status = 200, description = "OK", body=Vec<BookableSchemaAssignment>)))]
pub async fn list(
    Path(id): Path<i32>,
    Query(opts): Query<ListOptions>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let record = conditional_query_as!(
        BookableSchemaAssignment,
        r#"SELECT
            begins,
            schema_id
        FROM
            bookable_schema_assignments
        WHERE asset_id = {id}
            LIMIT {limit} OFFSET {offset}"#
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/bookables/{id}/schema_assignments", request_body = BookableSchemaAssignment, responses((status = 204, description = "Created")))] //no body
pub async fn post(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaAssignment>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "INSERT INTO bookable_schema_assignments (begins, schema_id, asset_id) VALUES ($1, $2, $3)",
        body.begins,
        body.schema_id,
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/bookables/{id}/schema_assignments/{begins}", responses((status = 204, description = "Deleted")))] //no body
pub async fn delete(
    Path((id, begins)): Path<(i32, DateTime<Utc>)>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "DELETE FROM bookable_schema_assignments WHERE begins = $1 AND asset_id = $2",
        begins,
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
