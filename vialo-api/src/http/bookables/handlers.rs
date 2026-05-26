use crate::helpers::{I18nMap, LangVariant, PgDateTime};

use super::models::{
    BoardPostIdModel, BookableAssetStatus, BookableAssetTranslatedAllLanguages,
    BookableAssetTranslatedWithStatus, BookableStatus,
};
use crate::http::util::{grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_member_of_group_or_app_role};
use crate::{
    helpers::grab_authd_conn_subsystem,
    http::util::{JsonE, User, VialoError},
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
use sqlx::{query, query_as};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use std::{i64, time::Duration};
use tokio::time::sleep;
use tracing::info;
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
    Extension(_user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    let re = query!("SELECT LOCALTIMESTAMP;").fetch_all(&data.db).await;
    println!("{:?}", re);
    // Execute the query and handle the result
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
            {#asset_type} ORDER BY id LIMIT {limit} OFFSET {offset}"#,
            #asset_type = match (opts.asset_types) {
                Some(at) => "WHERE asset_type_id = ANY({at:Vec<i32>})",
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

    info!("auth");
    let record = query_as!(BookableAssetStatus, r#"update bookable_assets set quick_unlock = tsrange(LOCALTIMESTAMP, LOCALTIMESTAMP + '1 minute', '[]') WHERE id = $1 AND (NOT (quick_unlock @> now()::timestamp) OR quick_unlock IS NULL) RETURNING $1 as "id!", asset_type_id as "asset_type_id!", 'quick_unlock'::bookable_status_type as "status!: BookableStatus",
    LOCALTIMESTAMP as begins,
   (LOCALTIMESTAMP + '1 minute') as "ends: PgDateTime";"#, id).fetch_one(&mut *conn).await?;
    info!("qry");

    let evil_data = data.clone();
    tokio::spawn(async move {
        evil_data
            .event_channel
            .broadcast(record.asset_type_id, record.clone())
            .await;
        if let (Some(begins), Some(PgDateTime::DateTime(ends))) = (record.begins, record.ends) {
            sleep(Duration::from_secs(
                (ends - begins).num_seconds().unsigned_abs() + 1,
            ))
            .await;
            if let Ok(record_2) = query_as!(
                BookableAssetStatus,
                r#"
                SELECT
                   bd.id as "id!",
                   bd.asset_type_id as "asset_type_id!",
                   bd.status as "status!: BookableStatus",
                   begins,
                   ends as "ends: PgDateTime"
               FROM
                bookable_asset_status bd WHERE id = $1;"#,
                id
            )
            .fetch_one(&mut *conn)
            .await
            {
                evil_data
                    .event_channel
                    .broadcast(record.asset_type_id, record_2)
                    .await;
            }
        }
    });

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PostBookableSchema {
    //pub group_id: Option<i32>,
    pub name: I18nMap,
    pub icon: Option<String>,
    pub slug: Option<String>,
    pub connector: Option<i32>,
    pub connector_output_id: Option<i32>,
    pub asset_type_id: i32,
}

#[utoipa::path(post, path = "/bookables", request_body=PostBookableSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post_bookable(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Member of the asset type's group or BookableManager may create assets.
    let group_id = sqlx::query_scalar!(
        "SELECT group_id FROM bookable_asset_types WHERE id = $1",
        body.asset_type_id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    check_member_of_group_or_app_role(user.clone(), group_id, AppRole::BookableManager, &data.db)
        .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // data.permission.check.check(CheckRequest {
    //     latest: true,
    //     tuple: Some(ketoapi::RelationTuple {
    //         namespace: "Bookables".into(),
    //         object: "1".into(),
    //         relation: "view".into(),
    //         subject: Some(Subject {
    //             r#ref: Some(Ref::Id("1".into())),
    //         }),
    //     }),
    //     ..Default::default()
    // });

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(body.name.into()))])
            .await
            .ok()
            .unwrap();

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
    .await?;

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
    // Must be a member of the existing asset's type's group (or BookableManager).
    // If reassigning to a new asset_type, must also be a member of that group.
    let existing_group_id = sqlx::query_scalar!(
        r#"SELECT bat.group_id FROM bookable_assets ba
         JOIN bookable_asset_types bat ON ba.asset_type_id = bat.id
         WHERE ba.id = $1"#,
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

    let new_group_id = sqlx::query_scalar!(
        "SELECT group_id FROM bookable_asset_types WHERE id = $1",
        body.asset_type_id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten()
    .ok_or(VialoError::NotFound())?;

    if new_group_id != existing_group_id {
        check_member_of_group_or_app_role(
            user.clone(),
            new_group_id,
            AppRole::BookableManager,
            &data.db,
        )
        .await?;
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(body.name.into()))])
            .await
            .ok()
            .unwrap();

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "UPDATE bookable_assets SET (name_i18n, icon, asset_type_id, slug, connector, connector_output_id) = ($1, $2, $3,$4,$5, $6) WHERE id = $7 RETURNING id",
        processed_i18n_fields.get("name"),
        body.icon,
        body.asset_type_id,
        body.slug,
        body.connector,
        body.connector_output_id,
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/bookables/{id}", params(BookableFilterOptions), responses((status = 200, description = "OK", body=LangVariant<BookableAssetTranslatedAllLanguages, BookableAssetTranslatedWithStatus>)))]
pub async fn get_bookable(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Query(opts): Query<BookableFilterOptions>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

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
                bookable_assets bp WHERE id = $1",
            id
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
               bookable_asset_status bd WHERE id = $2;"#,
            &langs,
            id
        )
        .fetch_one(&data.db)
        .await?;

        Ok(Json(LangVariant::Localized(post)))
    }
}
