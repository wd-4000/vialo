use super::super::util::grab_authd_conn_user;
use super::{
    boards::BoardPerm,
    models::{
        BoardPostIdModel, BoardPostModelTranslated, BoardPostModelTranslatedAllLanguages,
        BoardPostModelTranslatedWithPinnedOn,
    },
    schemas::{CreatePostSchema, PostContentFormat, PostFilterOptions},
};
use ammonia;
use pulldown_cmark::{Parser, html};

use crate::AppState;
use crate::helpers::LangVariant;
use crate::http::util::{JsonE, User, VialoError, get_i18n_arg_arrays, grab_trans};
use crate::permissions::{AppRole, check_app_role};
use axum::Extension;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use sqlx_conditional_queries::conditional_query_as;
use std::collections::HashMap;
use std::sync::Arc;

fn render_post(
    content: &Option<HashMap<String, String>>,
) -> (
    Option<HashMap<String, String>>,
    Option<HashMap<String, String>>,
) {
    let content_html: &Option<HashMap<String, String>> = &content.as_ref().map(|yes_content| {
        // option
        yes_content
            .iter()
            .map(|(k, v)| {
                // langs

                let parser = Parser::new(v);
                let mut html_buf = String::new();
                html::push_html(&mut html_buf, parser);
                let rendered_html = ammonia::clean(&html_buf);

                (k.clone(), rendered_html)
            })
            .collect()
    });

    let content_plain: &Option<HashMap<String, String>> =
        &content_html.as_ref().map(|yes_content| {
            // option
            yes_content
                .iter()
                .map(|(k, v)| {
                    // langs
                    let rendered_plain = ammonia::Builder::empty().clean(v).to_string();
                    (k.clone(), rendered_plain)
                })
                .collect()
        });

    (content_html.to_owned(), content_plain.to_owned())
}

#[utoipa::path(get, path = "/posts", params(PostFilterOptions), responses((status = 200, description = "OK", body=Vec<BoardPostModelTranslatedWithPinnedOn>)))]
pub async fn list_posts(
    Query(opts): Query<PostFilterOptions>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Some posts are only visible to logged-in users. TODO
    */
    let limit = opts.limit.unwrap_or(10);
    let langs = &(opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]));
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // This might be red in rust-analyzer. Ignore that.
    let posts = conditional_query_as!(BoardPostModelTranslatedWithPinnedOn, "SELECT * FROM (SELECT
                          bp.id,
                          bp.account_id,
                          bp.board_id,
                          bp.icon,
                          -- Use get_i18n_string for title, content, and location translations with fallback
                          get_i18n_string(bp.title, {langs}) AS title,
                          get_i18n_string(bp.content, {langs}) AS content,
                          get_i18n_string(bp.location, {langs}) AS location,
                          bp.event_from,
                          bp.event_to,
                          bp.created_at,
                          {#THROWING_UP} WHERE {#CURRENT_ONLY}
                          AND {#VISIBILITY}
                          AND {#BOARD}
                          GROUP BY bp.id
                         )  WHERE {#SEARCH}
                          ORDER BY (event_from <= now() AND event_to >= now()), created_at DESC LIMIT {limit} OFFSET {offset}",
                          #THROWING_UP = match(opts.pinned_on) {
                            Some(true)=> r#"array_agg(bpp.board_id) FILTER (WHERE bpp.board_id IS NOT NULL) AS pinned_on
                            FROM board_posts bp
                            LEFT JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
                            LEFT JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
                            LEFT JOIN board_pinned_posts bpp ON bpp.post_id = bp.id"#,
                            _ => r#"NULL AS "pinned_on: Vec<i32>"
                            FROM board_posts bp
                            LEFT JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
                            LEFT JOIN account_group_memberships agm ON bgp.group_id = agm.group_id"#
                          },
                          #SEARCH = match (opts.search){
                              Some(search) => "(title ILIKE '%' || {search} || '%' OR location ILIKE '%' || {search} || '%')",
                              _ => "TRUE"
                          },
                          #CURRENT_ONLY = match (opts.current_only){
                              Some(true) | None => "CASE WHEN bp.event_to IS NOT NULL THEN bp.event_to >= NOW() - interval '3 hours' ELSE bp.created_at > NOW() - interval '1 day' END",
                              Some(false) => "TRUE"
                          },
                          #VISIBILITY = match (user_o){
                              Some(User {id: uid} ) => "(bp.visibility IN ('public', 'logged_in') OR (bgp.perm >= 'view' AND agm.account_id = {uid}) OR account_role_exists({uid}, 'board_manager'::app_role))",
                              _ => "bp.visibility = 'public'"
                          },
                          #BOARD = match (&opts.board_id){
                              Some(board_ids) => "bp.board_id = ANY({board_ids})",
                              _ => "TRUE"
                          }).fetch_all(&data.db).await?;

    Ok((StatusCode::OK, Json(posts)))
}

/// Returns Ok(()) if the user may post to the given board:
/// they hold `board_perm >= post` via any of their groups, OR are a BoardManager.
async fn check_can_post_to_board(
    user: &User,
    board_id: i32,
    db: &sqlx::PgPool,
) -> Result<(), VialoError> {
    if check_app_role(user.clone(), AppRole::BoardManager, db)
        .await
        .is_ok()
    {
        return Ok(());
    }

    let allowed = sqlx::query_scalar!(
        r#"SELECT COUNT(*) > 0 AS "allowed: bool"
           FROM board_group_perms bgp
           JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
           WHERE bgp.board_id = $1
             AND agm.account_id = $2
             AND bgp.perm >= $3"#,
        board_id,
        user.id,
        BoardPerm::Post as BoardPerm,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if allowed {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

#[utoipa::path(post, path = "/posts", request_body = CreatePostSchema, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn add_post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreatePostSchema>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Posts can be posted by users whose group has >= post permission on the board,
       or by BoardManagers.
    */
    check_can_post_to_board(&user, body.board_id, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (content_html, content_plain) = render_post(&body.content);

    let processed_i18n_fields = super::super::util::insert_i18n_strings(
        &mut trans,
        vec![
            ("title", body.title),
            ("location", body.location),
            ("content", body.content),
            ("content_html", content_html),
            ("content_plain", content_plain),
        ],
    )
    .await?;

    let record = sqlx::query_as!(BoardPostIdModel,
        "INSERT INTO board_posts (account_id, board_id, icon, title, content, created_at, location, event_from, event_to, content_html, content_plain) VALUES ($1, $2, $3, $4, $5, NOW(), $6, $7, $8, $9, $10) RETURNING id",
       user.id, body.board_id, body.icon, processed_i18n_fields.get("title"), processed_i18n_fields.get("content"), processed_i18n_fields.get("location"), body.event_from, body.event_to, processed_i18n_fields.get("content_html"), processed_i18n_fields.get("content_plain")
    )
    .fetch_one(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/posts/{id}", request_body = CreatePostSchema, responses((status = 200, description = "Updated", body=BoardPostIdModel)))]
pub async fn update_post(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreatePostSchema>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Posts can be updated by:
       - the original author (requires board_perm >= post)
       - a moderator or admin of the post's board (board_perm >= moderate)
       - BoardManagers
    */
    let is_board_manager = check_app_role(user.clone(), AppRole::BoardManager, &data.db)
        .await
        .is_ok();

    if !is_board_manager {
        let allowed = sqlx::query_scalar!(
            r#"SELECT (
                -- author with post permission
                (
                    (SELECT COUNT(*) > 0 FROM board_posts WHERE id = $1 AND account_id = $2)
                    AND
                    (SELECT COUNT(*) > 0
                     FROM board_group_perms bgp
                     JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
                     WHERE bgp.board_id = $3 AND agm.account_id = $2 AND bgp.perm >= 'post')
                )
                OR
                -- moderator or admin of the post's board
                (SELECT COUNT(*) > 0
                 FROM board_posts bp
                 JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
                 JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
                 WHERE bp.id = $1 AND agm.account_id = $2 AND bgp.perm >= 'moderate')
            ) AS "allowed: bool""#,
            id,
            user.id,
            body.board_id,
        )
        .fetch_one(&data.db)
        .await
        .map_err(|e| VialoError::Anyhow(e.into()))?
        .unwrap_or(false);

        if !allowed {
            return Err(VialoError::Forbidden());
        }
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (title_langs, title_content) = get_i18n_arg_arrays(body.title);

    let (content_html, content_plain) = render_post(&body.content);
    let (content_html_langs, content_html_content) = get_i18n_arg_arrays(content_html);
    let (content_plain_langs, content_plain_content) = get_i18n_arg_arrays(content_plain);
    let (content_langs, content_content) = get_i18n_arg_arrays(body.content);

    let (location_langs, location_content) = get_i18n_arg_arrays(body.location);

    let record = sqlx::query_as!(BoardPostIdModel,
        "UPDATE board_posts SET (account_id, board_id, icon, title, content, created_at, location, event_from, event_to, content_html, content_plain) = ($1, $2, $3, update_i18n_strings(title, $4, $5), update_i18n_strings(content, $6, $7), NOW(), update_i18n_strings(location, $8, $9), $10, $11, update_i18n_strings(content_html, $13, $14), update_i18n_strings(content_plain, $15, $16)) WHERE id = $12 RETURNING id",
       user.id, body.board_id, body.icon, &title_langs, &title_content,  &content_langs,&content_content, &location_langs, &location_content, body.event_from, body.event_to, id, &content_html_langs,&content_html_content, &content_plain_langs,&content_plain_content
    )
    .fetch_one(&mut *trans)
    .await?;

    trans.commit().await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/posts/{id}", params(PostFilterOptions), responses((status = 200, description = "OK", body=LangVariant<BoardPostModelTranslatedAllLanguages, BoardPostModelTranslated>)))]
pub async fn get_post(
    Path(id): Path<i32>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
    Query(opts): Query<PostFilterOptions>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let wants_plain = opts.content_format.contains(&PostContentFormat::Plain);
    let wants_html = opts.content_format.contains(&PostContentFormat::Html);
    let wants_md = opts.content_format.is_empty()
        || opts.content_format.contains(&PostContentFormat::Markdown);

    let is_board_manager = if let Some(ref u) = user_o {
        check_app_role(u.clone(), AppRole::BoardManager, &data.db)
            .await
            .is_ok()
    } else {
        false
    };

    if langs == ["all"] {
        let post = conditional_query_as!(
            BoardPostModelTranslatedAllLanguages,
            "SELECT
                bp.id,
                bp.account_id,
                bp.board_id,
                bp.icon,
                -- Use get_i18n_string for title, content, and location translations with fallback
                get_i18n_all_string_translations(bp.title) AS title,
                {#CONTENT_MD},
                {#CONTENT_PLAIN},
                {#CONTENT_HTML},
                get_i18n_all_string_translations(bp.location) AS location,
                bp.event_from,
                bp.event_to,
                bp.created_at
            FROM
                board_posts bp
            LEFT JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
            LEFT JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
            WHERE bp.id = {id} AND {#VISIBILITY}",
                #CONTENT_PLAIN = match wants_plain {
                    true => "get_i18n_all_string_translations(bp.content_plain) AS content_plain",
                    false => "NULL::jsonb as content_plain"
                },
                #CONTENT_HTML = match wants_html {
                    true => "get_i18n_all_string_translations(bp.content_html) AS content_html",
                    false => "NULL::jsonb as content_html"
                },
                #CONTENT_MD = match wants_md {
                    true => "get_i18n_all_string_translations(bp.content) AS content",
                    false => "NULL::jsonb as content"
                },
                #VISIBILITY = match (is_board_manager, user_o.as_ref()){
                    (true, _) => "TRUE",
                    (false, Some(User {id: uid, ..} )) => "(bp.visibility IN ('public', 'logged_in') OR (bgp.perm >= 'view' AND agm.account_id = {uid}) OR account_role_exists({uid}, 'board_manager'::app_role))",
                    (false, _) => "bp.visibility = 'public'"
                }
        )
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::AllLangs(post)))
    } else {
        let langs = &langs;
        let post = conditional_query_as!(
            BoardPostModelTranslated,
            "SELECT
                bp.id,
                bp.account_id,
                bp.board_id,
                bp.icon,
                -- Use get_i18n_string for title, content, and location translations with fallback
                get_i18n_string(bp.title, {langs}) AS title,
                {#CONTENT_PLAIN},
                {#CONTENT_HTML},
                {#CONTENT_MD},
                get_i18n_string(bp.location, {langs}) AS location,
                bp.event_from,
                bp.event_to,
                bp.created_at
            FROM
                board_posts bp
            LEFT JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
            LEFT JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
            WHERE bp.id = {id} AND {#VISIBILITY}",
            #CONTENT_PLAIN = match wants_plain {
                true => "get_i18n_string(bp.content_plain, {langs}) AS content_plain",
                false => "NULL as content_plain"
            },
            #CONTENT_HTML = match wants_html {
                true => "get_i18n_string(bp.content_html, {langs}) AS content_html",
                false => "NULL as content_html"
            },
            #CONTENT_MD = match wants_md {
                true => "get_i18n_string(bp.content, {langs}) AS content",
                false => "NULL as content"
            },
            #VISIBILITY = match (is_board_manager, user_o.as_ref()){
                (true, _) => "TRUE",
                (false, Some(User {id: uid, ..} )) => "(bp.visibility IN ('public', 'logged_in') OR (bgp.perm >= 'view' AND agm.account_id = {uid}) OR account_role_exists({uid}, 'board_manager'::app_role))",
                (false, _) => "bp.visibility = 'public'"
            }
        )
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::Localized(post)))
    }
}

#[utoipa::path(delete, path = "/posts/{id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete_post(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Posts can be deleted by: the original author, a user whose group holds
       >= moderate permission on the post's board, or a BoardManager.
    */
    let is_board_manager = check_app_role(user.clone(), AppRole::BoardManager, &data.db)
        .await
        .is_ok();

    if !is_board_manager {
        let allowed = sqlx::query_scalar!(
            r#"SELECT (
                -- author
                (SELECT COUNT(*) > 0 FROM board_posts WHERE id = $1 AND account_id = $2)
                OR
                -- moderate permission on the post's board
                (SELECT COUNT(*) > 0
                 FROM board_posts bp
                 JOIN board_group_perms bgp ON bp.board_id = bgp.board_id
                 JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
                 WHERE bp.id = $1 AND agm.account_id = $2 AND bgp.perm >= 'moderate')
            ) AS "allowed: bool""#,
            id,
            user.id,
        )
        .fetch_one(&data.db)
        .await
        .map_err(|e| VialoError::Anyhow(e.into()))?
        .unwrap_or(false);

        if !allowed {
            return Err(VialoError::Forbidden());
        }
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let post = sqlx::query!(
        r#"DELETE FROM board_posts WHERE id = $1
        RETURNING array[title, content, location, content_html, content_plain] as "i18n_strings!""#,
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    sqlx::query!(
        "DELETE FROM i18n_strings WHERE id = ANY($1);",
        &post.i18n_strings
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}
