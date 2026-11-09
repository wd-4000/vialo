use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    middleware,
    response::IntoResponse,
    routing::{get, post, put},
};
use axum_extra::extract::Query;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, query, types::JsonValue};
use sqlx_conditional_queries::conditional_query_as;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AppState,
    helpers::{I18nMap, LangVariant, ResponseVariant, no},
    http::util::{
        self, JsonE, User, VialoError, clamp_pagination, get_i18n_arg_arrays, grab_authd_conn_user,
        grab_trans, insert_i18n_strings,
    },
    permissions::{AppRole, check_app_role, require_app_role},
};

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(add_app))
        .route("/{id}", put(put_app).delete(delete_app))
        .route("/order", put(put_app_order))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| {
                require_app_role(AppRole::SettingsManager, state, ext, req, next)
            },
        ))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_required,
        ))
        .route("/", get(list))
        .route("/{id}", get(get_app))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct App {
    pub id: i32,
    pub label: Option<String>,
    pub url: String,
    pub icon: Option<String>,
    pub index: i32,
    pub captive: bool,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct AppWithPerms {
    pub id: i32,
    pub label: Option<String>,
    pub url: String,
    pub icon: Option<String>,
    pub index: i32,
    pub captive: bool,
    /// NULL means everyone
    pub group_ids: Vec<Option<Uuid>>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct AppAllLanguages {
    pub id: i32,
    #[schema(value_type = Option<I18nMap>)]
    pub label: Option<JsonValue>,
    pub url: String,
    pub icon: Option<String>,
    pub index: i32,
    pub captive: bool,
    /// NULL means everyone
    pub group_ids: Vec<Option<Uuid>>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateAppSchema {
    pub label: I18nMap,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub url: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64", default)]
    pub icon: Option<String>,
    /// None appends on create and keeps the current position on update
    pub index: Option<i32>,
    pub captive: bool,
    /// NULL means everyone
    pub group_ids: Option<Vec<Option<Uuid>>>,
}

#[derive(Serialize, ToSchema)]
pub struct AppIdModel {
    pub id: i32,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct AppLangOptions {
    pub lang: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct AppFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    #[serde(default = "no")]
    pub include_group_perms: bool,
}

#[utoipa::path(get, path = "/apps", params(AppFilterOptions), responses((status = 200, description = "OK", body=ResponseVariant<Vec<App>, Vec<AppWithPerms>>)))]
pub async fn list(
    State(data): State<Arc<AppState>>,
    Query(opts): Query<AppFilterOptions>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let paginate = opts.limit.is_some() || opts.page.is_some();
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    if opts.include_group_perms {
        let Some(user) = user_o else {
            return Err(VialoError::AppError(
                StatusCode::UNAUTHORIZED,
                "unauthorized".to_string(),
            ));
        };
        check_app_role(user, AppRole::SettingsManager, &data.db).await?;

        let apps = conditional_query_as!(AppWithPerms,
        r#"SELECT a.id, get_i18n_string(a.label, {langs:Vec<String>}) AS label, a.url, a.icon, a.index, a.captive,
        COALESCE(ARRAY_AGG(ag.group_id) FILTER (WHERE ag.id IS NOT NULL), ARRAY[]::uuid[]) AS "group_ids!: Vec<Option<Uuid>>"
        FROM apps a LEFT JOIN app_group_perms ag ON a.id = ag.app_id GROUP BY a.id ORDER BY a.index, a.id {#pagination}"#,
        // opt-in because i don't know have a think
        #pagination = match (paginate) {
            true => "LIMIT {limit} OFFSET {offset}",
            false => "",
        }).fetch_all(&data.db).await?;
        Ok(Json(ResponseVariant::B(apps)))
    } else {
        let user_id = user_o.map(|u| u.id);
        let pagination = paginate.then_some((limit, offset));
        let apps = fetch_apps(&data.db, user_id, &langs, pagination).await?;
        Ok(Json(ResponseVariant::A(apps)))
    }
}

/// Apps visible to the given account/public ones if anonymous
pub async fn fetch_apps(
    db: impl sqlx::PgExecutor<'_>,
    user_id: Option<Uuid>,
    langs: &[String],
    pagination: Option<(i64, i64)>,
) -> sqlx::Result<Vec<App>> {
    // NULL group_id allows everyone to see the app
    conditional_query_as!(App,
    r#"SELECT a.id, get_i18n_string(a.label, {langs}) AS label, a.url, a.icon, a.index, a.captive
    FROM apps a JOIN app_group_perms ag ON a.id = ag.app_id
    LEFT JOIN account_group_memberships agm ON ag.group_id = agm.group_id AND agm.account_id = {user_id}
    WHERE ag.group_id IS NULL OR agm.account_id IS NOT NULL
    GROUP BY a.id ORDER BY a.index, a.id {#pagination}"#,
    #pagination = match pagination {
        Some((limit, offset)) => "LIMIT {limit} OFFSET {offset}",
        None => "",
    }).fetch_all(db).await
}

#[utoipa::path(get, path = "/apps/{id}", params(AppLangOptions), responses((status = 200, description = "OK", body=LangVariant<AppAllLanguages, AppWithPerms>)))]
pub async fn get_app(
    State(data): State<Arc<AppState>>,
    Query(opts): Query<AppLangOptions>,
    Extension(user_o): Extension<Option<User>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);
    let user_id = user_o.map(|u| u.id);

    // Settings managers see every app, everyone else only the ones granted to them
    if langs == ["all"] {
        let app = conditional_query_as!(AppAllLanguages,
        r#"SELECT a.id, get_i18n_all_string_translations(a.label) AS label, a.url, a.icon, a.index, a.captive,
        ARRAY(SELECT group_id FROM app_group_perms WHERE app_id = a.id) AS "group_ids!: Vec<Option<Uuid>>"
        FROM apps a WHERE a.id = {id} AND (account_role_exists({user_id}, 'settings_manager') OR EXISTS (
            SELECT 1 FROM app_group_perms ag
            LEFT JOIN account_group_memberships agm ON ag.group_id = agm.group_id AND agm.account_id = {user_id}
            WHERE ag.app_id = a.id AND (ag.group_id IS NULL OR agm.account_id IS NOT NULL)
        ))"#)
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::AllLangs(app)))
    } else {
        let app = conditional_query_as!(AppWithPerms,
        r#"SELECT a.id, get_i18n_string(a.label, {langs:Vec<String>}) AS label, a.url, a.icon, a.index, a.captive,
        ARRAY(SELECT group_id FROM app_group_perms WHERE app_id = a.id) AS "group_ids!: Vec<Option<Uuid>>"
        FROM apps a WHERE a.id = {id} AND (account_role_exists({user_id}, 'settings_manager') OR EXISTS (
            SELECT 1 FROM app_group_perms ag
            LEFT JOIN account_group_memberships agm ON ag.group_id = agm.group_id AND agm.account_id = {user_id}
            WHERE ag.app_id = a.id AND (ag.group_id IS NULL OR agm.account_id IS NOT NULL)
        ))"#)
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::Localized(app)))
    }
}

/// Replaces an app's permissions, none leaves the existing rows alone
async fn replace_app_permissions(
    db: &mut PgConnection,
    app_id: i32,
    group_ids: Option<&[Option<Uuid>]>,
) -> Result<(), VialoError> {
    let Some(group_ids) = group_ids else {
        return Ok(());
    };

    query!("DELETE FROM app_group_perms WHERE app_id = $1", app_id)
        .execute(&mut *db)
        .await?;

    if group_ids.is_empty() {
        return Ok(());
    }

    query!(
        "INSERT INTO app_group_perms (app_id, group_id)
        SELECT DISTINCT $1::int, group_id FROM unnest($2::uuid[]) AS p(group_id)",
        app_id,
        group_ids as &[Option<Uuid>]
    )
    .execute(&mut *db)
    .await?;

    Ok(())
}

#[utoipa::path(post, path = "/apps", request_body = CreateAppSchema, responses((status = 201, description = "Created", body=AppIdModel)))]
pub async fn add_app(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateAppSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields =
        insert_i18n_strings(&mut trans, vec![("label", Some(body.label))]).await?;

    let record = sqlx::query_as!(
        AppIdModel,
        r#"INSERT INTO apps (label, url, icon, index, captive)
        VALUES ($1, $2, $3, COALESCE($4, (SELECT COALESCE(MAX(index), 0) + 1 FROM apps)), $5) RETURNING id"#,
        processed_i18n_fields.get("label"),
        body.url,
        body.icon,
        body.index,
        body.captive
    )
    .fetch_one(&mut *trans)
    .await?;

    replace_app_permissions(&mut trans, record.id, body.group_ids.as_deref()).await?;

    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/apps/{id}", request_body = CreateAppSchema, responses((status = 200, description = "Updated", body=AppIdModel)))]
pub async fn put_app(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path(id): Path<i32>,
    JsonE(body): JsonE<CreateAppSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (label_langs, label_contents) = get_i18n_arg_arrays(Some(body.label));

    let record = sqlx::query_as!(
        AppIdModel,
        r#"UPDATE apps SET label=update_i18n_strings(label, $2, $3), url=$4, icon=$5, index=COALESCE($6, index), captive=$7 WHERE id = $1 RETURNING id"#,
        id,
        &label_langs,
        &label_contents,
        body.url,
        body.icon,
        body.index,
        body.captive,
    )
    .fetch_optional(&mut *trans)
    .await?
    .ok_or(VialoError::NotFound())?;

    replace_app_permissions(&mut trans, id, body.group_ids.as_deref()).await?;

    trans.commit().await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(put, path = "/apps/order", request_body = Vec<i32>, responses((status = 204, description = "Updated")))] // no body
pub async fn put_app_order(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(ids): JsonE<Vec<i32>>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // Must list every app exactly once
    let existing = sqlx::query_scalar!("SELECT id FROM apps ORDER BY id FOR UPDATE")
        .fetch_all(&mut *trans)
        .await?;
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    if sorted != existing {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "order_mismatch".into(),
        ));
    }

    sqlx::query!(
        r#"UPDATE apps a SET index = o.pos::int
        FROM unnest($1::int[]) WITH ORDINALITY AS o(id, pos) WHERE a.id = o.id"#,
        &ids
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/apps/{id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete_app(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let label = sqlx::query_scalar!("DELETE FROM apps WHERE id = $1 RETURNING label", id)
        .fetch_optional(&mut *trans)
        .await?
        .ok_or(VialoError::NotFound())?;
    query!("DELETE FROM i18n_strings WHERE id = $1", label)
        .execute(&mut *trans)
        .await?;

    trans.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
