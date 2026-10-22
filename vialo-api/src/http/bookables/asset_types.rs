use super::{handlers::BookableFilterOptions, models::BoardPostIdModel};
use crate::helpers::{I18nMap, LangVariant};
use crate::http::util::{JsonE, User, VialoError, clamp_pagination};
use crate::http::util::{grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_app_role};
use crate::{AppState, impl_jsonb_embed};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::query;
use sqlx::types::JsonValue;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct BookableAssetListTypeTranslated {
    pub id: i32,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableType {
    pub id: i32,
    #[schema(value_type = Option<I18nMap>)]
    pub name: Option<JsonValue>,
    pub permissions: Vec<BookableTypePermRow>,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAssetTypeTranslated {
    pub id: i32,
    pub name: Option<String>,
    pub permissions: Vec<BookableTypePermRow>,
}

#[derive(Clone, Debug, PartialEq, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "bookable_perm", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BookableTypePerm {
    View,
    Book,
    Admin,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct BookableTypePermRow {
    /// NULL means everyone that is logged in
    pub group_id: Option<Uuid>,
    pub perm: BookableTypePerm,
}

impl_jsonb_embed!(BookableTypePermRow);

#[utoipa::path(get, path = "/bookables/types", params(
    BookableFilterOptions
), responses((status = 200, description = "OK", body=Vec<BookableAssetListTypeTranslated>)))]
pub async fn list(
    Query(opts): Query<BookableFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let record = conditional_query_as!(
        BookableAssetListTypeTranslated,
        r#"SELECT id, name FROM (SELECT
                id,
                get_i18n_string(bd.name_i18n, {langs:Vec<String>}) AS name
            FROM
                bookable_asset_types bd) {#search} LIMIT {limit} OFFSET {offset}"#,
            #search = match opts.search {
                Some(search) => "WHERE name ILIKE '%' || {search} || '%'",
                None => "",
            }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PostBookableTypeSchema {
    pub name: I18nMap,
    pub permissions: Option<Vec<BookableTypePermRow>>,
}

/// Replaces an asset type's permissions. None leaves the existing rows alone.
async fn replace_asset_type_permissions(
    db: &mut PgConnection,
    asset_type_id: i32,
    permissions: Option<&[BookableTypePermRow]>,
) -> Result<(), VialoError> {
    let Some(permissions) = permissions else {
        return Ok(());
    };

    query!(
        "DELETE FROM bookable_asset_type_group_perms WHERE asset_type_id = $1",
        asset_type_id
    )
    .execute(&mut *db)
    .await?;

    if permissions.is_empty() {
        return Ok(());
    }

    let group_ids = permissions.iter().map(|p| p.group_id).collect::<Vec<_>>();
    let permissions = permissions.iter().map(|p| p.perm.clone()).collect::<Vec<_>>();

    query!("INSERT INTO bookable_asset_type_group_perms (asset_type_id, group_id, perm)
        SELECT $1 as asset_type_id, group_id, perm FROM unnest($2::uuid[], $3::bookable_perm[]) AS p(group_id, perm)",
        asset_type_id, &group_ids[..] as &[Option<Uuid>], &permissions[..] as &[BookableTypePerm])
    .execute(&mut *db)
    .await?;

    Ok(())
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

    replace_asset_type_permissions(&mut *db, id, body.permissions.as_deref()).await?;

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
            r#"SELECT
                bp.id,
                get_i18n_all_string_translations(bp.name_i18n) AS name,
                COALESCE(p.permissions, ARRAY[]::jsonb[]) AS "permissions!: Vec<BookableTypePermRow>"
            FROM
                bookable_asset_types bp
            LEFT JOIN LATERAL (
                SELECT array_agg(jsonb_build_object('group_id', batgp.group_id, 'perm', batgp.perm)) as permissions
                FROM bookable_asset_type_group_perms batgp
                WHERE batgp.asset_type_id = bp.id
            ) p ON true
                WHERE id = $1"#,
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
               get_i18n_string(bp.name_i18n, $1) AS name,
               COALESCE(p.permissions, ARRAY[]::jsonb[]) AS "permissions!: Vec<BookableTypePermRow>"
           FROM
               bookable_asset_types bp
           LEFT JOIN LATERAL (
               SELECT array_agg(jsonb_build_object('group_id', batgp.group_id, 'perm', batgp.perm)) as permissions
               FROM bookable_asset_type_group_perms batgp
               WHERE batgp.asset_type_id = bp.id
           ) p ON true
           WHERE id = $2;"#,
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

    replace_asset_type_permissions(&mut trans, id, body.permissions.as_deref()).await?;

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
