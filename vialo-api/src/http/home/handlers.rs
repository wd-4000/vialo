use super::super::home::models::JumboModelTranslated;
use super::{models::QuickLinkModelTranslated, schemas::PostFilterOptions};
use crate::http::home::models::BoardPostModelHomeTranslated;
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
pub struct HomePosts {
    pinned: Vec<BoardPostModelHomeTranslated>,
    timeline: Vec<BoardPostModelHomeTranslated>,
}

#[derive(Serialize, ToSchema)]
pub struct HomeResponse {
    quicklinks: Vec<QuickLinkModelTranslated>,
    jumbo: Vec<JumboModelTranslated>,
    posts: HomePosts,
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

    let langs = &langs;

    let posts = conditional_query_as!(
        BoardPostModelHomeTranslated,
        "SELECT
            bp.id,
            bp.icon,
            get_i18n_string(bp.title, {langs}) AS title,
            left(get_i18n_string(bp.content_html, {langs}), 400) AS content,
            get_i18n_string(bp.location, {langs}) AS location,
            bp.event_from,
            bp.event_to,
            bp.created_at,
            bpp.pinned_until
        FROM
            board_posts bp
        LEFT JOIN
            board_pinned_posts bpp ON bp.id = bpp.post_id
        WHERE ((bp.created_at >= CURRENT_DATE::timestamptz) OR (bpp.pinned_until <= NOW() AND bpp.board_id = 0))
          AND {#visibility}
        ORDER BY created_at DESC LIMIT {limit} OFFSET {offset}",
        #visibility = match &user_o {
            Some(User {id: uid}) => "(bp.visibility IN ('public', 'logged_in') OR account_board_perm_exists({uid}, bp.board_id, 'view'))",
            None => "bp.visibility = 'public'"
        }
    )
    .fetch_all(&data.db);

    let (jumbo, quicklinks, posts) = tokio::try_join!(jumbo, quicklinks, posts)?;

    let mut pinned = Vec::new();
    let mut timeline = Vec::new();

    for post in posts {
        if post.pinned_until.is_some() {
            pinned.push(post)
        } else {
            timeline.push(post)
        }
    }

    Ok((
        StatusCode::OK,
        Json(
            json!({"quicklinks":quicklinks, "jumbo":jumbo, "posts":{"pinned":pinned,"timeline":timeline}}),
        ),
    ))
}
