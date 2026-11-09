use super::super::util::grab_authd_conn_user;
use super::{
    boards::BoardPerm,
    models::{
        BoardPostIdModel, BoardPostModelTranslated, BoardPostModelTranslatedAllLanguages,
        BoardPostModelTranslatedWithPinnedOn, PostMediaModel, PostVisibility,
    },
    schemas::{CreatePostSchema, PostContentFormat, PostFilterOptions},
};
use ammonia;
use pulldown_cmark::{Parser, html};

use crate::AppState;
use crate::helpers::{I18nMap, LangVariant};
use crate::http::util::{
    JsonE, User, VialoError, clamp_pagination, get_i18n_arg_arrays, grab_trans,
};
use crate::media;
use axum::Extension;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;

fn render_post(content: &Option<I18nMap>) -> (Option<I18nMap>, Option<I18nMap>) {
    // Images are use the `media:<id>` schema and resolved at the client
    let mut sanitizer = ammonia::Builder::default();
    sanitizer.add_url_schemes(["media"]);

    let content_html: &Option<I18nMap> = &content.as_ref().map(|yes_content| {
        // option
        yes_content
            .iter()
            .map(|(k, v)| {
                // langs

                let parser = Parser::new(v);
                let mut html_buf = String::new();
                html::push_html(&mut html_buf, parser);
                let rendered_html = sanitizer.clean(&html_buf).to_string();

                (k.clone(), rendered_html)
            })
            .collect()
    });

    let content_plain: &Option<I18nMap> = &content_html.as_ref().map(|yes_content| {
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
       Logged-in users see 'public' and 'logged_in' posts (plus view-permissioned boards).
       Anonymous users see only 'public' posts.
    */
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let langs = &(opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]));

    let wants_plain = opts.content_format.contains(&PostContentFormat::Plain);
    let wants_html = opts.content_format.contains(&PostContentFormat::Html);
    let wants_md = opts.content_format.is_empty()
        || opts.content_format.contains(&PostContentFormat::Markdown);
    let uid = user_o.as_ref().map(|u| u.id);

    let pinned_on = opts.pinned_on.unwrap_or(false);
    let search = opts.search.as_deref();

    // #FILTER uses a conditional so the board/age predicates remain index-friendly
    // This might be red in rust-analyzer. Ignore that.
    let posts = conditional_query_as!(BoardPostModelTranslatedWithPinnedOn,
        r#"SELECT
            bp.id,
            bp.board_id,
            bp.icon,
            bp.cover_id,
            get_i18n_string(bp.title, {langs}) AS title,
            CASE WHEN {wants_md} THEN get_i18n_string(bp.content, {langs}) END AS content,
            CASE WHEN {wants_html} THEN get_i18n_string(bp.content_html, {langs}) END AS content_html,
            CASE WHEN {wants_plain} THEN get_i18n_string(bp.content_plain, {langs}) END AS content_plain,
            get_i18n_string(bp.location, {langs}) AS location,
            bp.event_from,
            bp.event_to,
            bp.created_at,
            ARRAY(SELECT jsonb_build_object('id', m.id, 'status', m.status, 'mime', m.mime, 'size_bytes', m.size_bytes,
                    'content', jsonb_build_object('type', m.type, 'width', m.width, 'height', m.height, 'variants', m.variants, 'filename', m.filename))
                FROM media m WHERE m.post_id = bp.id ORDER BY m.position) AS "media!: Vec<PostMediaModel>",
            CASE WHEN {pinned_on} THEN (SELECT array_agg(bpp.board_id) FROM board_pinned_posts bpp
                WHERE bpp.post_id = bp.id
                AND (bpp.pinned_until IS NULL OR bpp.pinned_until >= NOW())) END AS "pinned_on: Vec<i32>"
        FROM board_posts bp
        WHERE {#FILTER}
            -- visibility
            AND (bp.visibility = 'public'
                OR ({uid}::uuid IS NOT NULL AND (bp.visibility = 'logged_in' OR account_board_perm_exists({uid}, bp.board_id, 'view'::board_perm))))
            -- search
            AND ({search}::text IS NULL
                OR get_i18n_string(bp.title, {langs}) ILIKE '%' || {search} || '%'
                OR get_i18n_string(bp.location, {langs}) ILIKE '%' || {search} || '%')
        ORDER BY (bp.event_from <= now() AND bp.event_to >= now()), bp.created_at DESC
        LIMIT {limit} OFFSET {offset}"#,
        #FILTER = match (opts.current_only.unwrap_or(true), &opts.board_id) {
            (false, None) => "TRUE",
            (true, None) => r#"(bp.event_to >= NOW() - interval '3 hours'
                OR (bp.event_to IS NULL AND bp.created_at > NOW() - interval '1 day'))"#,
            // Is on a requested board, or pinned to one
            (false, Some(board_ids)) => r#"bp.id IN (
                SELECT pin.post_id FROM board_pinned_posts pin
                WHERE pin.board_id = ANY({board_ids}) AND (pin.pinned_until IS NULL OR pin.pinned_until >= NOW())
                UNION ALL
                SELECT h.id FROM board_posts h WHERE h.board_id = ANY({board_ids}))"#,
            // Pinned posts never age out
            (true, Some(board_ids)) => r#"bp.id IN (
                SELECT pin.post_id FROM board_pinned_posts pin
                WHERE pin.board_id = ANY({board_ids}) AND (pin.pinned_until IS NULL OR pin.pinned_until >= NOW())
                UNION ALL
                SELECT h.id FROM board_posts h WHERE h.board_id = ANY({board_ids})
                    AND (h.event_to >= NOW() - interval '3 hours'
                        OR (h.event_to IS NULL AND h.created_at > NOW() - interval '1 day')))"#
        },
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(posts)))
}

async fn check_can_post_to_board(
    user: &User,
    board_id: i32,
    db: &sqlx::PgPool,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT account_board_perm_exists($1, $2, $3) AS "allowed: bool""#,
        user.id,
        board_id,
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
    check_post_not_empty(&body)?;

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
        "INSERT INTO board_posts (account_id, board_id, icon, title, content, created_at, location, event_from, event_to, content_html, content_plain, visibility, cover_id) VALUES ($1, $2, $3, $4, $5, NOW(), $6, $7, $8, $9, $10, $11, $12) RETURNING id",
        user.id, body.board_id, body.icon, processed_i18n_fields.get("title"), processed_i18n_fields.get("content"), processed_i18n_fields.get("location"), body.event_from, body.event_to, processed_i18n_fields.get("content_html"), processed_i18n_fields.get("content_plain"), body.visibility as PostVisibility, body.cover_id
    )
    .fetch_one(&mut *trans)
    .await?;

    // A new post has nothing to drop
    super::media::sync_post_media(&mut trans, user.id, record.id, &body.media, body.cover_id)
        .await?;
    trans.commit().await?;

    if data.event_channels.posts_tx.send(record.id).is_err() {
        tracing::error!(
            post_id = record.id,
            "posts channel has no receivers, post email notifications will not be sent"
        );
        crate::health::add_health_event(
            &data.db,
            crate::http::history::models::Subsystem::Email,
            "channel_no_receivers",
            Some(serde_json::json!({"post_id": record.id})),
            20,
            false,
            None,
        )
        .await;
    }

    Ok((StatusCode::CREATED, Json(record)))
}

/// Title and content each need something in at least one language
fn check_post_not_empty(body: &CreatePostSchema) -> Result<(), VialoError> {
    let any_filled = |field: &Option<I18nMap>| {
        // The editor writes blank paragraphs as &nbsp
        field
            .iter()
            .flat_map(|m| m.values())
            .any(|v| !v.replace("&nbsp;", "").trim().is_empty())
    };
    if !any_filled(&body.title) {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing_title".into(),
        ));
    }
    if !any_filled(&body.content) {
        return Err(VialoError::AppError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing_content".into(),
        ));
    }
    Ok(())
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
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            -- can edit on current board: moderate perm, or author with post perm
            (
                account_board_perm_exists($1, bp.board_id, 'moderate'::board_perm)
                OR (bp.account_id = $1 AND account_board_perm_exists($1, bp.board_id, 'post'::board_perm))
            )
            -- if moving to a different board, also needs post perm there
            AND ($2 = bp.board_id OR account_board_perm_exists($1, $2, 'post'::board_perm))
        ) AS "allowed: bool"
        FROM board_posts bp WHERE bp.id = $3"#,
        user.id,
        body.board_id,
        id,
    )
    .fetch_one(&data.db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if !allowed {
        return Err(VialoError::Forbidden());
    }
    check_post_not_empty(&body)?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let (title_langs, title_content) = get_i18n_arg_arrays(body.title);

    let (content_html, content_plain) = render_post(&body.content);
    let (content_html_langs, content_html_content) = get_i18n_arg_arrays(content_html);
    let (content_plain_langs, content_plain_content) = get_i18n_arg_arrays(content_plain);
    let (content_langs, content_content) = get_i18n_arg_arrays(body.content);

    let (location_langs, location_content) = get_i18n_arg_arrays(body.location);

    let record = sqlx::query_as!(BoardPostIdModel,
        "UPDATE board_posts SET (board_id, icon, title, content, created_at, location, event_from, event_to, content_html, content_plain, visibility, cover_id) = ($1, $2, update_i18n_strings(title, $3, $4), update_i18n_strings(content, $5, $6), NOW(), update_i18n_strings(location, $7, $8), $9, $10, update_i18n_strings(content_html, $12, $13), update_i18n_strings(content_plain, $14, $15), $16, $17) WHERE id = $11 RETURNING id",
       body.board_id, body.icon, &title_langs, &title_content,  &content_langs,&content_content, &location_langs, &location_content, body.event_from, body.event_to, id, &content_html_langs,&content_html_content, &content_plain_langs,&content_plain_content, body.visibility as PostVisibility, body.cover_id
    )
    .fetch_one(&mut *trans)
    .await?;

    let dropped =
        super::media::sync_post_media(&mut trans, user.id, id, &body.media, body.cover_id).await?;
    trans.commit().await?;
    for media_id in dropped {
        media::remove_media_dir(&media::media_dir(&data.config.media.dir, media_id)).await;
    }
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

    let uid = user_o.as_ref().map(|u| u.id);

    if langs == ["all"] {
        let post = conditional_query_as!(
            BoardPostModelTranslatedAllLanguages,
            r#"SELECT
                bp.id,
                bp.board_id,
                bp.icon,
                bp.cover_id,
                get_i18n_all_string_translations(bp.title) AS title,
                CASE WHEN {wants_md} THEN get_i18n_all_string_translations(bp.content) END AS content,
                CASE WHEN {wants_html} THEN get_i18n_all_string_translations(bp.content_html) END AS content_html,
                CASE WHEN {wants_plain} THEN get_i18n_all_string_translations(bp.content_plain) END AS content_plain,
                get_i18n_all_string_translations(bp.location) AS location,
                bp.event_from,
                bp.event_to,
                bp.created_at,
                bp.visibility AS "visibility: PostVisibility",
                ARRAY(SELECT jsonb_build_object('id', m.id, 'status', m.status, 'mime', m.mime, 'size_bytes', m.size_bytes,
                    'content', jsonb_build_object('type', m.type, 'width', m.width, 'height', m.height, 'variants', m.variants, 'filename', m.filename))
                FROM media m WHERE m.post_id = bp.id ORDER BY m.position) AS "media!: Vec<PostMediaModel>",
                -- same rule as update_post
                COALESCE({uid}::uuid IS NOT NULL AND (
                    account_board_perm_exists({uid}, bp.board_id, 'moderate'::board_perm)
                    OR (bp.account_id = {uid} AND account_board_perm_exists({uid}, bp.board_id, 'post'::board_perm))
                ), false) AS "can_edit!"
            FROM board_posts bp
            WHERE bp.id = {id}
                -- visibility
                AND (bp.visibility = 'public'
                    OR ({uid}::uuid IS NOT NULL AND (bp.visibility = 'logged_in' OR account_board_perm_exists({uid}, bp.board_id, 'view'::board_perm))))"#,
        )
        .fetch_optional(&data.db)
        .await?
        .ok_or(VialoError::NotFound())?;

        Ok(Json(LangVariant::AllLangs(post)))
    } else {
        let langs = &langs;
        let post = conditional_query_as!(
            BoardPostModelTranslated,
            r#"SELECT
                bp.id,
                bp.board_id,
                bp.icon,
                bp.cover_id,
                get_i18n_string(bp.title, {langs}) AS title,
                CASE WHEN {wants_md} THEN get_i18n_string(bp.content, {langs}) END AS content,
                CASE WHEN {wants_html} THEN get_i18n_string(bp.content_html, {langs}) END AS content_html,
                CASE WHEN {wants_plain} THEN get_i18n_string(bp.content_plain, {langs}) END AS content_plain,
                get_i18n_string(bp.location, {langs}) AS location,
                bp.event_from,
                bp.event_to,
                bp.created_at,
                bp.visibility AS "visibility: PostVisibility",
                ARRAY(SELECT jsonb_build_object('id', m.id, 'status', m.status, 'mime', m.mime, 'size_bytes', m.size_bytes,
                    'content', jsonb_build_object('type', m.type, 'width', m.width, 'height', m.height, 'variants', m.variants, 'filename', m.filename))
                FROM media m WHERE m.post_id = bp.id ORDER BY m.position) AS "media!: Vec<PostMediaModel>",
                -- same rule as update_post
                COALESCE({uid}::uuid IS NOT NULL AND (
                    account_board_perm_exists({uid}, bp.board_id, 'moderate'::board_perm)
                    OR (bp.account_id = {uid} AND account_board_perm_exists({uid}, bp.board_id, 'post'::board_perm))
                ), false) AS "can_edit!"
            FROM board_posts bp
            WHERE bp.id = {id}
                -- visibility
                AND (bp.visibility = 'public'
                    OR ({uid}::uuid IS NOT NULL AND (bp.visibility = 'logged_in' OR account_board_perm_exists({uid}, bp.board_id, 'view'::board_perm))))"#,
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
    let allowed = sqlx::query_scalar!(
        r#"SELECT (
            bp.account_id = $2
            OR account_board_perm_exists($2, bp.board_id, 'moderate'::board_perm)
        ) AS "allowed: bool"
        FROM board_posts bp WHERE bp.id = $1"#,
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

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // Explicitly rather than by cascade, to learn which dirs to remove
    let media_ids = sqlx::query_scalar!("DELETE FROM media WHERE post_id = $1 RETURNING id", id)
        .fetch_all(&mut *trans)
        .await?;

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

    for media_id in media_ids {
        media::remove_media_dir(&media::media_dir(&data.config.media.dir, media_id)).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
