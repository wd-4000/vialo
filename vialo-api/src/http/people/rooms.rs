use super::{
    models::RoomModel,
    schemas::{CreateRoomSchema, UserFilterOptions},
};
use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use sqlx::query_as;
use std::sync::Arc;
use uuid::Uuid;

#[utoipa::path(post, path = "/people/rooms", request_body=CreateRoomSchema, responses((status = 201, description = "Created", body=RoomModel)))]
pub async fn add_room(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateRoomSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let created_room = sqlx::query_as!(
        RoomModel,
        "INSERT INTO res_rooms (label, capacity, floor) VALUES ($1, $2, $3) RETURNING *",
        body.label,
        body.capacity,
        body.floor
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::CREATED, Json(created_room)))
}

#[utoipa::path(get, path = "/people/rooms", params(UserFilterOptions), responses((status = 200, description = "OK", body=Vec<RoomModel>)))]
pub async fn list_rooms(
    Query(opts): Query<UserFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let search = opts.search.unwrap_or("".to_string());

    let record = query_as!(
        RoomModel,
        "SELECT * FROM res_rooms WHERE label ILIKE '%' || $1 || '%' LIMIT $2 OFFSET $3",
        search,
        limit,
        offset
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(put, path = "/people/rooms/{id}", request_body=CreateRoomSchema, responses((status = 200, description = "Updated", body=RoomModel)))]
pub async fn put_room(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    JsonE(body): JsonE<CreateRoomSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let created_room = sqlx::query_as!(
        RoomModel,
        "UPDATE res_rooms SET (label, capacity, floor) = ($1, $2, $3) WHERE id = $4 RETURNING *",
        body.label,
        body.capacity,
        body.floor,
        id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::OK, Json(created_room)))
}

#[utoipa::path(delete, path = "/people/rooms/{id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete_room(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    sqlx::query!("DELETE FROM res_rooms WHERE id = $1", id)
        .execute(&mut *conn)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/people/rooms/{id}", responses((status = 200, description = "OK", body=RoomModel)))]
pub async fn get_room(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let record = query_as!(RoomModel, "SELECT * FROM res_rooms WHERE id = $1", id)
        .fetch_one(&data.db)
        .await?;

    Ok((StatusCode::OK, Json(record)))
}
