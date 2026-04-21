use super::{handlers::BookableFilterOptions, models::BoardPostIdModel};
use crate::AppState;
use crate::helpers::LangVariant;
use crate::http::util::{JsonE, User, VialoError};
use crate::http::util::{grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_app_role, check_member_of_group_or_app_role};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::query;
use sqlx::types::JsonValue;
use sqlx_conditional_queries::conditional_query_as;
use std::collections::HashMap;
use std::i64;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize)]
pub struct BookableAssetTypeTranslated {
    pub id: i32,
    pub name: Option<String>,
    pub group_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/bookables/types", params(
    BookableFilterOptions
), responses((status = 200, description = "OK")))]
pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableAssetTypeTranslated,
        r#"SELECT
            id,
            get_i18n_string(bd.name_i18n, {langs:Vec<String>}) AS name,
            bd.group_id
        FROM
            bookable_asset_types bd  LIMIT {limit} OFFSET {offset}"#
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PostBookableTypeSchema {
    //pub group_id: Option<i32>,
    pub name: HashMap<String, String>,
    pub group_id: Uuid,
}

#[utoipa::path(post, path = "/bookables/types", request_body = PostBookableTypeSchema, responses((status = 201, description = "Created")))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableTypeSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Only a BookableManager may create new asset types.
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(body.name))])
            .await
            .ok()
            .unwrap();

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO bookable_asset_types (name_i18n, group_id) VALUES ($1, $2) RETURNING id",
        processed_i18n_fields.get("name"),
        body.group_id
    )
    .fetch_one(&mut *trans)
    .await?;

    let device_response = record;
    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(device_response)))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BookableType {
    //pub group_id: Option<i32>,
    pub id: i32,
    pub name: Option<JsonValue>,
    pub group_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/bookables/types/{id}", params(BookableFilterOptions), responses((status = 200, description = "OK")))]
pub async fn get(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Query(opts): Query<BookableFilterOptions>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    if langs == ["all"] {
        let post = sqlx::query_as!(
            BookableType,
            "SELECT
                bp.id,
                group_id,
                get_i18n_all_string_translations(bp.name_i18n) AS name
            FROM
                bookable_asset_types bp WHERE id = $1",
            id
        )
        .fetch_one(&data.db)
        .await?;

        Ok(Json(LangVariant::AllLangs(post)))
    } else {
        let post = sqlx::query_as!(
            BookableAssetTypeTranslated,
            r#"SELECT
            bp.id,
            group_id,
               get_i18n_string(bp.name_i18n, $1) AS name

           FROM
                bookable_asset_types bp WHERE id = $2;"#,
            &langs,
            id
        )
        .fetch_one(&data.db)
        .await?;

        Ok(Json(LangVariant::Localized(post)))
    }
}

#[utoipa::path(put, path = "/bookables/types/{id}", request_body = PostBookableTypeSchema, responses((status = 200, description = "Updated")))]
pub async fn put(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableTypeSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // BookableManager or any member of the type's group may edit it.
    check_member_of_group_or_app_role(
        user.clone(),
        body.group_id,
        AppRole::BookableManager,
        &data.db,
    )
    .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(body.name))])
            .await
            .ok()
            .unwrap();

    sqlx::query!(
        r#"UPDATE bookable_asset_types SET (name_i18n, group_id) = ($1, $2) WHERE id = $3"#,
        processed_i18n_fields.get("name"),
        body.group_id,
        id
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/bookables/types/{id}", responses((status = 200, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    // BookableManager or any member of the type's group may delete it.
    let group_id = sqlx::query_scalar!(
        "SELECT group_id FROM bookable_asset_types WHERE id = $1",
        id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    check_member_of_group_or_app_role(user.clone(), group_id, AppRole::BookableManager, &data.db)
        .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let d = query!(
        r#"DELETE FROM bookable_asset_types WHERE id = $1 RETURNING name_i18n "#,
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    query!(r#"DELETE FROM i18n_strings WHERE id = $1;"#, d.name_i18n)
        .execute(&mut *trans)
        .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}
