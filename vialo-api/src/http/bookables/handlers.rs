use crate::helpers::{I18nMap, LangVariant, PgDateTime};

use super::models::{
    BoardPostIdModel, BookableAssetStatus, BookableAssetTranslatedAllLanguages,
    BookableAssetTranslatedWithStatus, BookableStatus,
};
use super::permissions::{BookablePerm, require_asset_type_perm};
use crate::http::util::{clamp_pagination, grab_authd_conn_user, grab_trans};
use crate::{
    helpers::grab_authd_conn_subsystem,
    http::util::{JsonE, User, VialoError, is_unique_violation},
};
// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::AppState;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::query_as;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use std::{i64, time::Duration};
use tokio::time::sleep;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct BookableFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub asset_types: Option<Vec<i32>>,
}

#[utoipa::path(get, path = "/bookables", params(BookableFilterOptions), responses((status = 200, description = "OK", body=Vec<BookableAssetTranslatedWithStatus>)))]
pub async fn list_bookables(
    Query(opts): Query<BookableFilterOptions>,
    State(data): State<Arc<AppState>>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let user_id_o = user_o.as_ref().map(|u| u.id);

    let record = conditional_query_as!(
            BookableAssetTranslatedWithStatus,
            r#"SELECT
                id as "id!",
                icon,
                get_i18n_string(name_i18n, {langs:Vec<String>}) AS name,
                asset_type_id as "asset_type_id!",
                status as "status!: BookableStatus",
                begins,
                ends as "ends: PgDateTime"
            FROM
                bookable_asset_status
            WHERE account_bookable_perm_exists({user_id_o}, asset_type_id, 'view'::bookable_perm)
            {#asset_type} ORDER BY id LIMIT {limit} OFFSET {offset}"#,
            #asset_type = match (opts.asset_types) {
                Some(at) => "AND asset_type_id = ANY({at:Vec<i32>})",
                None => "",
            },
    )
    .fetch_all(&data.db)
    .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(post, path = "/bookables/{id}/quick-unlock", responses((status = 204, description = "Unlocked")))] // no body
pub async fn quick_unlock(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = if let Some(user) = user_o {
        grab_authd_conn_user(&data.db, user.id).await?
    } else {
        grab_authd_conn_subsystem(&data.db, "guest").await?
    };

    let record = query_as!(BookableAssetStatus, r#"update bookable_assets set quick_unlock = tstzrange(now(), now() + '1 minute', '[]') WHERE id = $1 AND (NOT (quick_unlock @> now()) OR quick_unlock IS NULL) RETURNING $1 as "id!", asset_type_id as "asset_type_id!", 'quick_unlock'::bookable_status_type as "status!: BookableStatus",
    now() as begins,
   (now() + '1 minute') as "ends: PgDateTime";"#, id).fetch_one(&mut *conn).await?;

    let evil_data = data.clone();
    tokio::spawn(async move {
        evil_data
            .event_channels
            .bookables
            .broadcast(record.asset_type_id, record.id, record.clone())
            .await;
        if let (Some(begins), Some(PgDateTime::DateTime(ends))) = (record.begins, record.ends) {
            sleep(Duration::from_secs(
                (ends - begins).num_seconds().unsigned_abs() + 1,
            ))
            .await;
            crate::bookables::fetch_and_broadcast_status(&evil_data, [id]).await;
        }
    });

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PostBookableSchema {
    pub name: I18nMap,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64")]
    pub icon: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_128")]
    pub slug: Option<String>,
    pub connector: Option<i32>,
    pub connector_output_id: Option<i32>,
    pub asset_type_id: i32,
    pub schema_id: Option<i32>,
}

#[utoipa::path(post, path = "/bookables", request_body=PostBookableSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post_bookable(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableSchema>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm(
        Some(user.id),
        body.asset_type_id,
        BookablePerm::Admin,
        &data.db,
    )
    .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(body.name.into()))])
            .await?;

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO bookable_assets (name_i18n, icon, asset_type_id, slug, connector, connector_output_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        processed_i18n_fields.get("name"),
        body.icon,
        body.asset_type_id,
        body.slug,
        body.connector,
        body.connector_output_id,
    )
    .fetch_one(&mut *trans)
    .await
    .map_err(|e| if is_unique_violation(&e) {
        VialoError::AppError(StatusCode::CONFLICT, "slug_conflict".into())
    } else { e.into() })?;

    if let Some(schema_id) = body.schema_id {
        sqlx::query!(
            "INSERT INTO bookable_schema_assignments (begins, schema_id, asset_id) VALUES (CURRENT_DATE, $1, $2)",
            schema_id,
            record.id,
        )
        .execute(&mut *trans)
        .await?;
    }

    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/bookables/{id}", request_body=PostBookableSchema, responses((status = 200, description = "Updated", body=BoardPostIdModel)))]
pub async fn put_bookable(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let current_type_id = sqlx::query_scalar!(
        "SELECT asset_type_id FROM bookable_assets WHERE id = $1 FOR UPDATE",
        id
    )
    .fetch_one(&mut *trans)
    .await?;
    require_asset_type_perm(
        Some(user.id),
        current_type_id,
        BookablePerm::Admin,
        &data.db,
    )
    .await?;
    if body.asset_type_id != current_type_id {
        require_asset_type_perm(
            Some(user.id),
            body.asset_type_id,
            BookablePerm::Admin,
            &data.db,
        )
        .await?;
    }

    let (name_langs, name_contents) =
        super::super::util::get_i18n_arg_arrays(Some(body.name.into()));

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "UPDATE bookable_assets SET name_i18n = update_i18n_strings(name_i18n, $1, $2), icon = $3, asset_type_id = $4, slug = $5, connector = $6, connector_output_id = $7 WHERE id = $8 RETURNING id",
        &name_langs,
        &name_contents,
        body.icon,
        body.asset_type_id,
        body.slug,
        body.connector,
        body.connector_output_id,
        id
    )
    .fetch_one(&mut *trans)
    .await
    .map_err(|e| if is_unique_violation(&e) {
        VialoError::AppError(StatusCode::CONFLICT, "slug_conflict".into())
    } else { e.into() })?;

    trans.commit().await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/bookables/{id}", params(BookableFilterOptions), responses((status = 200, description = "OK", body=LangVariant<BookableAssetTranslatedAllLanguages, BookableAssetTranslatedWithStatus>)))]
pub async fn get_bookable(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Query(opts): Query<BookableFilterOptions>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let user_id_o = user_o.as_ref().map(|u| u.id);

    if langs == ["all"] {
        let post = sqlx::query_as!(
            BookableAssetTranslatedAllLanguages,
            "SELECT
                bp.id,
                icon,
                slug,
                connector,
                connector_output_id,
                bp.asset_type_id,
                get_i18n_all_string_translations(bp.name_i18n) AS name
            FROM
                bookable_assets bp
            WHERE bp.id = $1
              AND account_bookable_perm_exists($2, bp.asset_type_id, 'view'::bookable_perm)",
            id,
            user_id_o
        )
        .fetch_one(&data.db)
        .await?;

        Ok(Json(LangVariant::AllLangs(post)))
    } else {
        let post = sqlx::query_as!(
            BookableAssetTranslatedWithStatus,
            r#"SELECT
               bd.id as "id!",
               icon,
               get_i18n_string(bd.name_i18n, $1) AS name,
               bd.asset_type_id as "asset_type_id!",
               bd.status as "status!: BookableStatus",
               begins,
               ends as "ends: PgDateTime"
           FROM
               bookable_asset_status bd
           WHERE bd.id = $2
             AND account_bookable_perm_exists($3, bd.asset_type_id, 'view'::bookable_perm)"#,
            &langs,
            id,
            user_id_o
        )
        .fetch_one(&data.db)
        .await?;

        Ok(Json(LangVariant::Localized(post)))
    }
}
