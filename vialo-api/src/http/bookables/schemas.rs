use super::models::BoardPostIdModel;
use crate::AppState;
use crate::http::util::grab_authd_conn_user;
use crate::http::util::{JsonE, ListOptions, User, VialoError};
use crate::permissions::{AppRole, check_member_of_group_or_app_role};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableSchema {
    pub id: i32,
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type: Option<i32>,
    pub slot_price: Option<i32>,
}

#[utoipa::path(get, path = "/bookables/schemas", responses((status = 200, description = "OK")))]
pub async fn list(
    Query(opts): Query<ListOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);

    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableSchema,
        r#"SELECT
            id,
            label,
            schedule::text[] as "schedule!",
            asset_type,
            slot_price
        FROM
            bookable_schemas LIMIT {limit} OFFSET {offset}"#
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/bookables/schemas/{id}", responses((status = 200, description = "OK")))]
pub async fn get(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableSchema,
        r#"SELECT
            id,
            label,
            schedule::text[] as "schedule!",
            asset_type,
            slot_price
        FROM
            bookable_schemas
        WHERE
            id = {id}"#,
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(delete, path = "/bookables/schemas/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let group_id = sqlx::query_scalar!(
        r#"SELECT bat.group_id FROM bookable_schemas bs
         JOIN bookable_asset_types bat ON bs.asset_type = bat.id
         WHERE bs.id = $1"#,
        id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten();

    match group_id {
        Some(group_id) => {
            check_member_of_group_or_app_role(
                user.clone(),
                group_id,
                AppRole::BookableManager,
                &data.db,
            )
            .await?;
        }
        // No resolvable group (schema has no asset_type, or the type has no group):
        // fall back to requiring BookableManager.
        None => {
            crate::permissions::check_app_role(user.clone(), AppRole::BookableManager, &data.db)
                .await?;
        }
    }

    // Execute the query and handle the result
    let _record = query!(
        r#"DELETE FROM
            bookable_schemas
        WHERE
            id = $1"#,
        id
    )
    .execute(&data.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableSchemaPostOrPut {
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type: Option<i32>,
    pub slot_price: Option<i32>,
}

#[utoipa::path(post, path = "/bookables/schemas", responses((status = 201, description = "Created")))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    // Member of the target asset type's group or BookableManager may create schemas.
    if let Some(asset_type_id) = body.asset_type {
        let group_id = sqlx::query_scalar!(
            "SELECT group_id FROM bookable_asset_types WHERE id = $1",
            asset_type_id
        )
        .fetch_optional(&data.db)
        .await?
        .flatten()
        .ok_or(VialoError::NotFound())?;

        check_member_of_group_or_app_role(
            user.clone(),
            group_id,
            AppRole::BookableManager,
            &data.db,
        )
        .await?;
    } else {
        // No asset_type means no group to check against; require BookableManager.
        crate::permissions::check_app_role(user.clone(), AppRole::BookableManager, &data.db)
            .await?;
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO bookable_schemas (label, schedule, asset_type, slot_price) VALUES ($1, $2::time[], $3, $4) RETURNING id",
        body.label, body.schedule as Vec<String>, body.asset_type, body.slot_price
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/bookables/schemas/{id}", responses((status = 200, description = "Updated")))]
pub async fn put(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    // Must be a member of the existing schema's asset type's group (or BookableManager).
    // If reassigning to a new asset_type, must also be a member of that group.
    let existing_group_id = sqlx::query_scalar!(
        r#"SELECT bat.group_id FROM bookable_schemas bs
         JOIN bookable_asset_types bat ON bs.asset_type = bat.id
         WHERE bs.id = $1"#,
        id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    check_member_of_group_or_app_role(
        user.clone(),
        existing_group_id,
        AppRole::BookableManager,
        &data.db,
    )
    .await?;

    if let Some(new_asset_type_id) = body.asset_type {
        let new_group_id = sqlx::query_scalar!(
            "SELECT group_id FROM bookable_asset_types WHERE id = $1",
            new_asset_type_id
        )
        .fetch_optional(&data.db)
        .await?
        .flatten()
        .ok_or(VialoError::NotFound())?;

        check_member_of_group_or_app_role(
            user.clone(),
            new_group_id,
            AppRole::BookableManager,
            &data.db,
        )
        .await?;
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let record = sqlx::query_as!(
        BookableSchema,
        r#"UPDATE bookable_schemas SET (label, schedule, asset_type, slot_price) = ($1, $2::time[], $3, $4) WHERE id = $5
        RETURNING
        id,
        label,
        schedule::text[] as "schedule!",
        asset_type,
        slot_price"#,
        body.label, body.schedule as Vec<String>, body.asset_type, body.slot_price, id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
