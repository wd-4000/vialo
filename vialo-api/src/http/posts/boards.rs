use super::super::util::grab_authd_conn_user;
use super::models::PostVisibility;
use super::permissions::require_board_perm;
use super::schemas::PostFilterOptions;
use crate::helpers::{I18nMap, LangVariant};
use crate::http::posts::models::BoardPostIdModel;
use crate::http::util::models::GroupEmbed;
use crate::http::util::{JsonE, User, VialoError, clamp_pagination, grab_trans, is_unique_violation};
use crate::permissions::check_manager_of_group;
use crate::AppState;
use axum::Extension;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::{query_as, types::JsonValue};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "board_perm", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum BoardPerm {
    View,
    Post,
    Moderate,
    Admin,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateBoardSchema {
    pub group_id: Uuid,
    pub label: I18nMap,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_128")]
    pub slug: String,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64")]
    pub icon: Option<String>,

    pub default_post_visibility: PostVisibility,
}

#[derive(Serialize, ToSchema)]
pub struct BoardModelTranslatedEmbeddedGroup {
    pub id: i32,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub slug: Option<String>,
    pub group: Option<GroupEmbed>,
    pub default_post_visibility: PostVisibility,
}

#[derive(Serialize, ToSchema)]
pub struct BoardModelTranslated {
    pub group_id: Option<Uuid>,
    pub label: Option<String>,
    pub icon: Option<String>,

    pub default_post_visibility: PostVisibility,

    pub slug: String,
}

#[derive(Serialize, ToSchema)]
pub struct BoardModelTranslatedAllLanguages {
    pub group_id: Option<Uuid>,
    #[schema(value_type = Option<I18nMap>)]
    pub label: Option<JsonValue>,
    pub icon: Option<String>,

    pub default_post_visibility: PostVisibility,

    pub slug: String,
}

#[utoipa::path(get, path = "/posts/boards", params(PostFilterOptions), responses((status = 200, description = "OK", body=Vec<BoardModelTranslatedEmbeddedGroup>)))]
pub async fn list_boards(
    Query(opts): Query<PostFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let boards = query_as!(
        BoardModelTranslatedEmbeddedGroup,
        r#"SELECT
            bd.id,
            bd.slug,
            bd.icon,
            get_i18n_string(bd.label, $1) AS label,
            CASE
                WHEN bd.group_id IS NOT NULL THEN jsonb_build_object(
                    'id', bd.group_id,
                    'label', ag.label
                )
                ELSE NULL
            END AS "group: GroupEmbed",
            bd.default_post_visibility AS "default_post_visibility: PostVisibility"
        FROM
            boards bd LEFT JOIN account_groups ag ON bd.group_id = ag.id
        WHERE account_board_perm_exists($4, bd.id, 'view')
        LIMIT $2 OFFSET $3"#,
        &langs,
        limit as i64,
        offset as i64,
        user.id,
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(boards)))
}

#[utoipa::path(get, path = "/posts/boards/{id}", params(PostFilterOptions), responses((status = 200, description = "OK", body=LangVariant<BoardModelTranslatedAllLanguages, BoardModelTranslated>)))]
pub async fn get_board(
    Query(opts): Query<PostFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    Path(id_or_slug): Path<String>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    if langs == ["all"] {
        let uid = user.id;
        let post = conditional_query_as!(
                    BoardModelTranslatedAllLanguages,
                    r#"SELECT
            get_i18n_all_string_translations(bb.label) AS label,
            bb.group_id,
            bb.icon,
            bb.slug,
            bb.default_post_visibility AS "default_post_visibility: PostVisibility"
            FROM boards bb {#SELECTID} AND account_board_perm_exists({uid}, bb.id, 'view')"#,
        #SELECTID = match id_or_slug.parse::<i32>() {
            Ok(id_int) => "WHERE id = {id_int}",
            _ => "WHERE slug = {id_or_slug}"
        }
                )
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::AllLangs(post)))
    } else {
        let uid = user.id;
        let post = conditional_query_as!(
            BoardModelTranslated,
            r#"SELECT
                bb.group_id,
                bb.slug,
                bb.icon,
                bb.default_post_visibility AS "default_post_visibility: PostVisibility",
                get_i18n_string(bb.label, {langs:Vec<String>}) AS label
            FROM
                boards bb {#SELECTID} AND account_board_perm_exists({uid}, bb.id, 'view')"#,
        #SELECTID = match id_or_slug.parse::<i32>() {
            Ok(id_int) => "WHERE id = {id_int}",
            _ => "WHERE slug = {id_or_slug}"
        })
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::Localized(post)))
    }
}

#[derive(Serialize, ToSchema)]
pub struct BoardPermissions {
    pub group_id: Option<Uuid>,
    pub label: Option<String>,
    pub perm: BoardPerm,
}


#[utoipa::path(get, path = "/posts/boards/{id}/permissions", params(PostFilterOptions), responses((status = 200, description = "OK", body=Vec<BoardPermissions>)))]
pub async fn get_permissions(
    Query(opts): Query<PostFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, VialoError> {
    require_board_perm(user.id,id, BoardPerm::Admin, &data.db).await?;

    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let res = query_as!(
        BoardPermissions,
        r#"SELECT bgp.group_id, ag.label, bgp.perm as "perm!: BoardPerm" from board_group_perms bgp LEFT JOIN account_groups ag ON bgp.group_id = ag.id WHERE board_id = $1 LIMIT $2 OFFSET $3"#,
        id, limit, offset).fetch_all(&data.db).await?;

    Ok((StatusCode::OK, Json(res)))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PermSchema {
    pub perm: BoardPerm,
}

#[utoipa::path(put, path = "/posts/boards/{board_id}/permissions/{group_id}", request_body = PermSchema, responses((status = 204, description = "Updated")))]
pub async fn put_permission(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path((board_id, group_id)): Path<(i32, Uuid)>,
    JsonE(body): JsonE<PermSchema>,
) -> Result<impl IntoResponse, VialoError> {
    require_board_perm(user.id,board_id, BoardPerm::Admin, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        r#"INSERT INTO board_group_perms (board_id, group_id, perm) VALUES ($1, $2, $3) ON CONFLICT (group_id, board_id) DO UPDATE SET perm = $3"#,
        board_id,
        group_id,
        body.perm as BoardPerm
    )
    .execute(&mut *conn)
    .await?;

    Ok((StatusCode::NO_CONTENT, {}))
}

#[utoipa::path(delete, path = "/posts/boards/{board_id}/permissions/{group_id}", responses((status = 204, description = "Deleted")))]
pub async fn delete_permission(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path((board_id, group_id)): Path<(i32, Uuid)>,
) -> Result<impl IntoResponse, VialoError> {
    require_board_perm(user.id,board_id, BoardPerm::Admin, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        r#"DELETE FROM board_group_perms WHERE board_id = $1 AND group_id = $2"#,
        board_id,
        group_id
    )
    .execute(&mut *conn)
    .await?;

    Ok((StatusCode::NO_CONTENT, {}))
}

#[utoipa::path(post, path = "/posts/boards", request_body = CreateBoardSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn add_board(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateBoardSchema>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Boards can be created by users that are managers of the group they want to link the board to.
    */

    check_manager_of_group(user.clone(), body.group_id, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let processed_i18n_fields = super::super::util::insert_i18n_strings(
        &mut trans,
        vec![("label", Some(body.label.into()))],
    )
    .await?;

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO boards (label, group_id, default_post_visibility, slug, icon) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        processed_i18n_fields.get("label"),
        body.group_id,
        body.default_post_visibility as PostVisibility,
        body.slug,
        body.icon
    )
    .fetch_one(&mut *trans)
    .await
    .map_err(|e| if is_unique_violation(&e) {
        VialoError::AppError(StatusCode::CONFLICT, "slug_conflict".into())
    } else { e.into() })?;

    let device_response = record;
    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(device_response)))
}
#[utoipa::path(put, path = "/posts/boards/{id}", request_body = CreateBoardSchema, responses((status = 200, description = "Updated", body=BoardPostIdModel)))]
pub async fn put_board(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path(id): Path<i32>,
    JsonE(body): JsonE<CreateBoardSchema>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Boards can be modified by users that are managers of both the group they want to link the board to,
       and the original group.
    */
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            account_role_exists($1, 'board_manager')
            OR (
                EXISTS (SELECT 1 FROM account_group_memberships WHERE account_id = $1 AND group_id = $2 AND role = 'manager')
                AND EXISTS (SELECT 1 FROM boards bd JOIN account_group_memberships am ON bd.group_id = am.group_id WHERE bd.id = $3 AND am.account_id = $1 AND am.role = 'manager')
            )
        ) AS "allowed: bool""#,
        user.id,
        body.group_id,
        id,
    )
    .fetch_one(&data.db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);
    if !allowed {
        return Err(VialoError::Forbidden());
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (label_langs, label_contents) =
        super::super::util::get_i18n_arg_arrays(Some(body.label.into()));

    let record = sqlx::query_as!(
        BoardPostIdModel,
        r#"UPDATE boards SET label=update_i18n_strings(label, $2, $3), group_id=$4, default_post_visibility=$5, slug=$6, icon=$7 WHERE id = $1 RETURNING id"#,
        id,
        &label_langs,
        &label_contents,
        body.group_id,
        body.default_post_visibility as PostVisibility,
        body.slug,
        body.icon,
    )
    .fetch_one(&mut *trans)
    .await
    .map_err(|e| if is_unique_violation(&e) {
        VialoError::AppError(StatusCode::CONFLICT, "slug_conflict".into())
    } else { e.into() })?;

    let device_response = record;
    trans.commit().await?;
    Ok((StatusCode::OK, Json(device_response)))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PinPostSchema {
    pub post_id: i32,
    pub pinned_until: chrono::DateTime<chrono::Utc>,
}
#[utoipa::path(post, path = "/posts/boards/{id}/pin", request_body = PinPostSchema, responses((status = 201, description = "Created")))] // no body
pub async fn pin_post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path(id): Path<i32>,
    JsonE(body): JsonE<PinPostSchema>,
) -> Result<impl IntoResponse, VialoError> {
    require_board_perm(user.id,id, BoardPerm::Moderate, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        r#"INSERT INTO board_pinned_posts (board_id, post_id, pinned_until) VALUES ($1, $2, $3)"#,
        id,
        body.post_id,
        body.pinned_until
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::CREATED)
}

#[utoipa::path(delete, path = "/posts/boards/{board_id}/pin/{post_id}", responses((status = 204, description = "Deleted")))]
pub async fn unpin_post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Path((board_id, post_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, VialoError> {
    require_board_perm(user.id,board_id, BoardPerm::Moderate, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        r#"DELETE FROM board_pinned_posts WHERE board_id = $1 AND post_id = $2"#,
        board_id,
        post_id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
