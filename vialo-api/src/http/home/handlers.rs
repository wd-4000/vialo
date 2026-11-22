use super::super::home::models::JumboModelTranslated;
use super::{models::QuickLinkModelTranslated, schemas::PostFilterOptions};
use crate::http::posts::models::{
    BoardPostModelTranslatedWithPinnedOn, BoardPostsResponse, PostMediaModel,
};
use crate::http::util::{User, VialoError, clamp_pagination};
use crate::{AppState, list_i18n_generic};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use serde::Serialize;
use serde_json::json;
use sqlx::query_as;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[utoipa::path(get, path = "/home/quicklinks", params(PostFilterOptions), responses((status = 200, description = "OK", body=Vec<QuickLinkModelTranslated>)))]
pub async fn list_quicklinks(
    Query(opts): Query<PostFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    list_i18n_generic!(
        &data.db,
        r#"SELECT id as "id!", label, link FROM get_i18n_quicklinks($1) LIMIT $2 OFFSET $3"#,
        opts,
        QuickLinkModelTranslated
    );
}

#[utoipa::path(get, path = "/home/jumbo", params(PostFilterOptions), responses((status = 200, description = "OK", body=Vec<JumboModelTranslated>)))]
pub async fn list_jumbo(
    Query(opts): Query<PostFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    list_i18n_generic!(
        &data.db,
        r#"SELECT id as "id!", img, headline, title, content, link FROM get_i18n_jumbo($1) LIMIT $2 OFFSET $3"#,
        opts,
        JumboModelTranslated
    );
}

#[derive(Serialize, ToSchema)]
pub struct HomeResponse {
    quicklinks: Vec<QuickLinkModelTranslated>,
    groups: Vec<HomeGroup>,
    jumbo: Vec<JumboModelTranslated>,
    /// Split by pinning on the home board (0)
    posts: BoardPostsResponse,
}

#[derive(Serialize, Debug, Clone, PartialEq, ToSchema)]
pub struct HomeGroup {
    pub id: Uuid,
    pub label: String,
    pub icon: Option<String>,
}

#[utoipa::path(get, path = "/home",  params(PostFilterOptions), responses((status = 200, description = "OK", body = HomeResponse)))]
pub async fn get_home_aggregated(
    Query(opts): Query<PostFilterOptions>,
    State(data): State<Arc<AppState>>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let jumbo = query_as!(
        JumboModelTranslated,
        r#"SELECT id as "id!", img, headline, title, content, link FROM get_i18n_jumbo($1) LIMIT $2 OFFSET $3"#,
        &langs,
        limit,
        offset
    )
    .fetch_all(&data.db);

    let quicklinks = query_as!(
        QuickLinkModelTranslated,
        r#"SELECT id as "id!", label, link FROM get_i18n_quicklinks($1) LIMIT $2 OFFSET $3"#,
        &langs,
        limit,
        offset
    )
    .fetch_all(&data.db);

    let groups = query_as!(
        HomeGroup,
        r#"SELECT id, label, icon FROM account_groups WHERE public = true ORDER BY index, id LIMIT $1 OFFSET $2"#,
        limit,
        offset
    )
    .fetch_all(&data.db);

    let langs = &langs;

    // Today's posts from any board + posts pinned on the home board (0), pinned first
    let posts = conditional_query_as!(
        BoardPostModelTranslatedWithPinnedOn,
        r#"SELECT
            bp.id,
            bp.board_id,
            bp.icon,
            bp.cover_id,
            get_i18n_string(bp.title, {langs}) AS title,
            NULL::text AS content,
            NULL::text AS content_html,
            left(get_i18n_string(bp.content_plain, {langs}), 400) AS content_plain,
            get_i18n_string(bp.location, {langs}) AS location,
            bp.event_from,
            bp.event_to,
            bp.created_at,
            ARRAY(SELECT jsonb_build_object('id', m.id, 'status', m.status, 'mime', m.mime, 'size_bytes', m.size_bytes,
                    'content', jsonb_build_object('type', m.type, 'width', m.width, 'height', m.height, 'variants', m.variants, 'filename', m.filename))
                FROM media m WHERE m.post_id = bp.id ORDER BY m.position) AS "media!: Vec<PostMediaModel>",
            pins.boards AS "pinned_on: Vec<i32>"
        FROM board_posts bp
        LEFT JOIN LATERAL (
            SELECT array_agg(pin.board_id) AS boards,
                max(pin.pin_priority) FILTER (WHERE pin.board_id = 0) AS priority
            FROM board_pinned_posts pin
            WHERE pin.post_id = bp.id AND (pin.pinned_until IS NULL OR pin.pinned_until >= NOW())
        ) pins ON TRUE
        WHERE bp.id IN (
                SELECT pin.post_id FROM board_pinned_posts pin
                WHERE pin.board_id = 0 AND (pin.pinned_until IS NULL OR pin.pinned_until >= NOW())
                UNION ALL
                SELECT h.id FROM board_posts h WHERE h.created_at >= CURRENT_DATE::timestamptz)
          AND {#visibility}
        ORDER BY pins.priority DESC NULLS LAST, bp.created_at DESC
        LIMIT {limit} OFFSET {offset}"#,
        #visibility = match &user_o {
            Some(User {id: uid, ..}) => "(bp.visibility IN ('public', 'logged_in') OR account_board_perm_exists({uid}, bp.board_id, 'view'))",
            None => "bp.visibility = 'public'"
        }
    )
    .fetch_all(&data.db);

    let (jumbo, quicklinks, posts, groups) = tokio::try_join!(jumbo, quicklinks, posts, groups)?;

    let (pinned, timeline): (Vec<_>, Vec<_>) = posts
        .into_iter()
        .partition(|p| p.pinned_on.as_ref().is_some_and(|b| b.contains(&0)));
    let posts = BoardPostsResponse {
        pinned: pinned.into_iter().map(Into::into).collect(),
        timeline: timeline.into_iter().map(Into::into).collect(),
    };

    Ok((
        StatusCode::OK,
        Json(json!({"quicklinks":quicklinks, "jumbo":jumbo, "posts":posts, "groups":groups})),
    ))
}
