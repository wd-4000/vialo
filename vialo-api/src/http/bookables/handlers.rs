use crate::helpers::{I18nMap, LangVariant, PgDateTime};

use super::asset_types::{PostBookableTypeSchema, insert_asset_type};
use super::models::{
    BoardPostIdModel, BookableAssetQueue, BookableAssetTranslatedAllLanguages,
    BookableAssetTranslatedWithStatus, BookableStatus,
};
use super::permissions::{
    BookableCaller, BookablePerm, require_asset_type_perm, require_asset_type_perm_by_asset,
};
use super::schemas::{NewSchemaInline, insert_schema};
use crate::http::util::{clamp_pagination, grab_authd_conn_user, grab_trans};
use crate::permissions::{AppRole, check_app_role};
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
use sqlx::{query, query_scalar};
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
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
    caller: BookableCaller,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let (user_id_o, kiosk_types) = match caller {
        BookableCaller::Kiosk(kiosk) => (None, kiosk.asset_types),
        BookableCaller::Account(id) => (Some(id), vec![]),
        BookableCaller::Anonymous => (None, vec![]),
    };

    let record = conditional_query_as!(
            BookableAssetTranslatedWithStatus,
            r#"SELECT
                id as "id!",
                icon,
                get_i18n_string(name_i18n, {langs:Vec<String>}) AS name,
                asset_type_id as "asset_type_id!",
                status as "status!: BookableStatus",
                begins,
                ends as "ends: PgDateTime",
                appointment_id
            FROM
                bookable_asset_status
            WHERE (account_bookable_perm_exists({user_id_o}, asset_type_id, 'view'::bookable_perm) OR asset_type_id = ANY({kiosk_types:Vec<i32>}))
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

// rooms list
#[utoipa::path(get, path = "/bookables/queues", params(BookableFilterOptions), responses((status = 200, description = "OK", body=Vec<BookableAssetQueue>)))]
pub async fn list_queues(
    Query(opts): Query<BookableFilterOptions>,
    State(data): State<Arc<AppState>>,
    caller: BookableCaller,
) -> Result<impl IntoResponse, VialoError> {
    let (account_id, kiosk_types) = match caller {
        BookableCaller::Kiosk(kiosk) => (None, Some(kiosk.asset_types)),
        BookableCaller::Account(id) => (Some(id), None),
        BookableCaller::Anonymous => return Err(VialoError::Forbidden()),
    };

    let rows = query!(
        r#"SELECT
            q.asset_id as "asset_id!",
            q.asset_type_id as "asset_type_id!",
            q.appointment_id as "appointment_id!",
            q.begins,
            q.ends as "ends: PgDateTime",
            q.room,
            q.maintenance as "maintenance!",
            q.bucket as "bucket!"
        FROM bookable_asset_queue q
        WHERE ((q.bucket = 'previous' AND q.past_rank <= $1)
           OR q.bucket = 'current'
           OR (q.bucket = 'upcoming' AND q.future_rank <= $2))
          AND ($3::uuid IS NULL OR account_bookable_perm_exists($3, q.asset_type_id, 'book'::bookable_perm))
          AND ($4::int[] IS NULL OR q.asset_type_id = ANY($4))
          AND ($5::int[] IS NULL OR q.asset_type_id = ANY($5))
        ORDER BY q.asset_id, q.begins"#,
        crate::bookables::QUEUE_PREVIOUS_DEPTH,
        crate::bookables::QUEUE_UPCOMING_DEPTH,
        account_id,
        kiosk_types.as_deref(),
        opts.asset_types.as_deref(),
    )
    .fetch_all(&data.db)
    .await?;

    Ok((
        StatusCode::OK,
        Json(crate::bookables::assemble_queues(
            rows.into_iter()
                .map(|r| crate::bookables::QueueRow {
                    asset_id: r.asset_id,
                    asset_type_id: r.asset_type_id,
                    appointment_id: r.appointment_id,
                    begins: r.begins,
                    ends: r.ends,
                    room: r.room,
                    maintenance: r.maintenance,
                    bucket: r.bucket,
                })
                .collect(),
        )),
    ))
}

#[utoipa::path(post, path = "/bookables/{id}/quick-unlock", responses((status = 204, description = "Unlocked")))] // no body
pub async fn quick_unlock(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    caller: BookableCaller,
) -> Result<impl IntoResponse, VialoError> {
    // Either a valid kiosk credential, an account with 'book' or nothing.
    let mut conn = match caller {
        BookableCaller::Kiosk(kiosk) => {
            let asset_type_id = query_scalar!(
                "SELECT asset_type_id FROM bookable_assets WHERE id = $1",
                id
            )
            .fetch_optional(&data.db)
            .await?
            .ok_or(VialoError::NotFound())?;

            if !kiosk.allows(asset_type_id) {
                return Err(VialoError::Forbidden());
            }

            grab_authd_conn_subsystem(&data.db, "bookable").await?
        }
        BookableCaller::Account(user_id) => {
            require_asset_type_perm_by_asset(user_id, id, BookablePerm::Book, &data.db).await?;

            grab_authd_conn_user(&data.db, user_id).await?
        }
        BookableCaller::Anonymous => {
            return Err(VialoError::Forbidden());
        }
    };

    let res = query!(
        "UPDATE bookable_assets SET quick_unlock = tstzrange(now(), now() + '1 minute', '[]') WHERE id = $1 AND (NOT (quick_unlock @> now()) OR quick_unlock IS NULL)",
        id
    )
    .execute(&mut *conn)
    .await?;

    if res.rows_affected() != 1 {
        return Err(VialoError::NotFound());
    }

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
    /// Exactly one of asset_type_id / new_asset_type must be set.
    pub asset_type_id: Option<i32>,
    pub new_asset_type: Option<PostBookableTypeSchema>,
    /// At most one of schema_id / new_schema may be set.
    pub schema_id: Option<i32>,
    pub new_schema: Option<NewSchemaInline>,
}

/// Update carries no schema fields; those go through POST /bookables/{id}/schema_assignments.
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PutBookableSchema {
    pub name: I18nMap,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64")]
    pub icon: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_128")]
    pub slug: Option<String>,
    pub connector: Option<i32>,
    pub connector_output_id: Option<i32>,
    pub asset_type_id: i32,
}

enum AssetTypeInput {
    Existing(i32),
    New(PostBookableTypeSchema),
}

#[utoipa::path(post, path = "/bookables", request_body=PostBookableSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post_bookable(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let PostBookableSchema {
        name,
        icon,
        slug,
        connector,
        connector_output_id,
        asset_type_id,
        new_asset_type,
        schema_id,
        new_schema,
    } = body;

    let asset_type = match (asset_type_id, new_asset_type) {
        (Some(id), None) => AssetTypeInput::Existing(id),
        (None, Some(t)) => AssetTypeInput::New(t),
        _ => {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "exactly one of asset_type_id or new_asset_type must be set".into(),
            ));
        }
    };

    if schema_id.is_some() && new_schema.is_some() {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "schema_id and new_schema are mutually exclusive".into(),
        ));
    }

    // An existing type is gated per-type; creating one needs the global role.
    match &asset_type {
        AssetTypeInput::Existing(id) => {
            require_asset_type_perm(Some(user.id), *id, BookablePerm::Admin, &data.db).await?
        }
        AssetTypeInput::New(_) => {
            check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?
        }
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let asset_type_id = match asset_type {
        AssetTypeInput::Existing(id) => id,
        AssetTypeInput::New(t) => insert_asset_type(&mut trans, t).await?,
    };

    let processed_i18n_fields =
        super::super::util::insert_i18n_strings(&mut trans, vec![("name", Some(name.into()))])
            .await?;

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO bookable_assets (name_i18n, icon, asset_type_id, slug, connector, connector_output_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        processed_i18n_fields.get("name"),
        icon,
        asset_type_id,
        slug,
        connector,
        connector_output_id,
    )
    .fetch_one(&mut *trans)
    .await
    .map_err(|e| if is_unique_violation(&e) {
        VialoError::AppError(StatusCode::CONFLICT, "slug_conflict".into())
    } else { e.into() })?;

    let schema_id = match new_schema {
        Some(s) => Some(insert_schema(&mut trans, s, asset_type_id).await?),
        None => schema_id,
    };

    if let Some(schema_id) = schema_id {
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

#[utoipa::path(put, path = "/bookables/{id}", request_body=PutBookableSchema, responses((status = 200, description = "Updated", body=BoardPostIdModel)))]
pub async fn put_bookable(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PutBookableSchema>,
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
               ends as "ends: PgDateTime",
               bd.appointment_id
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
