use super::{handlers::BookableFilterOptions, models::BoardPostIdModel};
use crate::AppState;
use crate::helpers::{I18nMap, LangVariant};
use crate::http::util::{JsonE, User, VialoError, clamp_pagination};
use crate::http::util::{grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_app_role};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::query;
use sqlx::types::JsonValue;
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct BookableAssetTypeTranslated {
    pub id: i32,
    pub name: Option<String>,
}

#[utoipa::path(get, path = "/bookables/types", params(
    BookableFilterOptions
), responses((status = 200, description = "OK", body=Vec<BookableAssetTypeTranslated>)))]
pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    if let Some(search) = opts.search {
        let record = conditional_query_as!(
            BookableAssetTypeTranslated,
            r#"SELECT id, name FROM (SELECT
                id,
                get_i18n_string(bd.name_i18n, {langs:Vec<String>}) AS name
            FROM
                bookable_asset_types bd) WHERE name ILIKE '%' || {search} || '%' LIMIT {limit} OFFSET {offset}"#
        )
        .fetch_all(&data.db)
        .await?;

        Ok((StatusCode::OK, Json(record)))
    } else {
        let record = conditional_query_as!(
            BookableAssetTypeTranslated,
            r#"SELECT
                id,
                get_i18n_string(bd.name_i18n, {langs:Vec<String>}) AS name
            FROM
                bookable_asset_types bd LIMIT {limit} OFFSET {offset}"#
        )
        .fetch_all(&data.db)
        .await?;

        Ok((StatusCode::OK, Json(record)))
    }
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PostBookableTypeSchema {
    pub name: I18nMap,
}

/// Inserts an asset type and its name, returning its id. The caller checks permissions.
pub async fn insert_asset_type(
    db: &mut PgConnection,
    body: PostBookableTypeSchema,
) -> Result<i32, VialoError> {
    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut *db, vec![("name", Some(body.name.into()))])
            .await?;

    let id = sqlx::query_scalar!(
        "INSERT INTO bookable_asset_types (name_i18n) VALUES ($1) RETURNING id",
        processed_i18n_fields.get("name")
    )
    .fetch_one(&mut *db)
    .await?;

    Ok(id)
}

#[utoipa::path(post, path = "/bookables/types", request_body = PostBookableTypeSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableTypeSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Only a BookableManager may create new asset types.
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let id = insert_asset_type(&mut trans, body).await?;

    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(BoardPostIdModel { id })))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct BookableType {
    pub id: i32,
    #[schema(value_type = Option<I18nMap>)]
    pub name: Option<JsonValue>,
}

#[utoipa::path(get, path = "/bookables/types/{id}", params(BookableFilterOptions), responses((status = 200, description = "OK", body = LangVariant<BookableType, BookableAssetTypeTranslated>)))]
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

#[utoipa::path(put, path = "/bookables/types/{id}", request_body = PostBookableTypeSchema, responses((status = 204, description = "Updated")))] // no body
pub async fn put(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableTypeSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (name_langs, name_contents) =
        super::super::util::get_i18n_arg_arrays(Some(body.name.into()));

    sqlx::query!(
        r#"UPDATE bookable_asset_types SET name_i18n = update_i18n_strings(name_i18n, $1, $2) WHERE id = $3"#,
        &name_langs,
        &name_contents,
        id
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/bookables/types/{id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
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
