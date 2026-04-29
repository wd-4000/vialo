use crate::permissions::{AppRole, check_manager_of_group_or_app_role};
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
    http::util::{JsonE, ListOptions, User, VialoError, grab_authd_conn_user},
};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct BookableSchemaAssignment {
    pub begins: DateTime<Utc>,
    pub schema_id: i32,
}

#[utoipa::path(get, path = "/bookables/{asset_id}/schema_assignments", params(ListOptions), responses((status = 200, description = "OK", body=Vec<BookableSchemaAssignment>)))]
pub async fn list(
    Path(asset_id): Path<i32>,
    Query(opts): Query<ListOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);

    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableSchemaAssignment,
        r#"SELECT
            begins,
            schema_id
        FROM
            bookable_schema_assignments
        WHERE asset_id = {asset_id}
            LIMIT {limit} OFFSET {offset}"#
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/bookables/{asset_id}/schema_assignments", request_body = BookableSchemaAssignment, responses((status = 204, description = "Created")))] //no body
pub async fn post(
    Path(asset_id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaAssignment>,
) -> Result<impl IntoResponse, VialoError> {
    // BookableManager or manager of the asset's type's group may assign schemas.
    let group_id = sqlx::query_scalar!(
        r#"SELECT bat.group_id FROM bookable_assets ba
         JOIN bookable_asset_types bat ON ba.asset_type = bat.id
         WHERE ba.id = $1"#,
        asset_id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::BookableManager, &data.db)
        .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "INSERT INTO bookable_schema_assignments (begins, schema_id, asset_id) VALUES ($1, $2, $3)",
        body.begins,
        body.schema_id,
        asset_id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/bookables/{asset_id}/schema_assignments/{begins}", responses((status = 204, description = "Deleted")))] //no body
pub async fn delete(
    Path((asset_id, begins)): Path<(i32, DateTime<Utc>)>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    // BookableManager or manager of the asset's type's group may remove schema assignments.
    let group_id = sqlx::query_scalar!(
        r#"SELECT bat.group_id FROM bookable_assets ba
         JOIN bookable_asset_types bat ON ba.asset_type = bat.id
         WHERE ba.id = $1"#,
        asset_id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::BookableManager, &data.db)
        .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "DELETE FROM bookable_schema_assignments WHERE begins = $1 AND asset_id = $2",
        begins,
        asset_id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
