use std::{io::ErrorKind, sync::Arc};

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderName, StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{delete, get, post},
};
use axum_extra::extract::Query;
use futures::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use utoipa::IntoParams;
use uuid::Uuid;

pub mod models;

use crate::{
    AppState,
    http::util::{self, User, VialoError, grab_authd_conn_user, grab_trans},
    media::{
        self, ORIGINAL,
        detect::{self, Detected, SNIFF_LEN},
        models::{JobData, MediaContext, MediaModel, MediaRow, MediaStatus, MediaType},
        process::{self, ProbeError},
    },
};

/// Unattached post uploads per user
const MAX_STAGED: i64 = 50;
const MB: u64 = 1024 * 1024;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(upload_media))
        .route("/{id}", delete(delete_media))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_required,
        ))
        .route("/{id}", get(get_media))
        .route("/{id}/file", get(serve_file))
        .route("/{id}/{size}", get(serve_image))
}

fn io(e: std::io::Error) -> VialoError {
    VialoError::Anyhow(e.into())
}

fn app_error(status: StatusCode, code: &str) -> VialoError {
    VialoError::AppError(status, code.into())
}

struct Visible {
    row: MediaRow,
    public: bool,
}

/// post: same visibility
/// staged: owner only
/// avatars: logged in
async fn fetch_visible(
    db: &sqlx::PgPool,
    id: Uuid,
    user: Option<&User>,
) -> Result<Visible, VialoError> {
    let uid = user.map(|u| u.id);
    let r = sqlx::query!(
        r#"SELECT m.id, m.type AS "media_type: MediaType", m.context AS "context: MediaContext", m.mime,
            m.filename, m.size_bytes, m.status AS "status: MediaStatus", m.post_id, m.position,
            m.width, m.height, m.variants,
            COALESCE(bp.visibility = 'public', false) AS "public!",
            COALESCE(CASE
                WHEN m.context = 'avatar' THEN $2::uuid IS NOT NULL
                WHEN m.post_id IS NULL THEN m.owner_id = $2
                ELSE bp.visibility = 'public' OR ($2::uuid IS NOT NULL AND (bp.visibility = 'logged_in'
                    OR account_board_perm_exists($2, bp.board_id, 'view'::board_perm)))
            END, false) AS "allowed!"
        FROM media m LEFT JOIN board_posts bp ON bp.id = m.post_id WHERE m.id = $1"#,
        id,
        uid
    )
    .fetch_optional(db)
    .await?
    .filter(|r| r.allowed)
    .ok_or(VialoError::NotFound())?;

    Ok(Visible {
        public: r.public,
        row: MediaRow {
            id: r.id,
            media_type: r.media_type,
            context: r.context,
            mime: r.mime,
            filename: r.filename,
            size_bytes: r.size_bytes,
            status: r.status,
            post_id: r.post_id,
            position: r.position,
            width: r.width,
            height: r.height,
            variants: r.variants,
        },
    })
}

/// Only public posts go to shared caches
fn cache_control(public: bool) -> &'static str {
    if public {
        "public, max-age=86400"
    } else {
        "private, max-age=31536000, immutable"
    }
}

#[derive(Deserialize, IntoParams)]
pub struct UploadOptions {
    pub context: MediaContext,
    /// Download name for documents; the extension is corrected to match the content
    pub filename: Option<String>,
}

struct Limits {
    image: u64,
    document: u64,
    quota_left: u64,
}

impl Limits {
    fn for_type(&self, t: Option<MediaType>) -> u64 {
        match t {
            Some(MediaType::Image) => self.image,
            Some(MediaType::Document) => self.document,
            None => self.image.max(self.document),
        }
    }
}

#[utoipa::path(post, path = "/media", params(UploadOptions),
    request_body(content = Vec<u8>, content_type = "application/octet-stream",
        description = "Images: jpeg, png, webp (+heif/avif with media-heif). Documents: pdf, docx/xlsx/pptx, odt/ods/odp"),
    responses(
        (status = 201, description = "Document stored", body = MediaModel),
        (status = 202, description = "Image stored, encoding queued", body = MediaModel)))]
pub async fn upload_media(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    Query(opts): Query<UploadOptions>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Any user. Post uploads stay private to the uploader until attached to a post they can edit.
       Avatars become the uploader's own once encoded.
    */
    let config = &data.config.media;
    let usage = sqlx::query!(
        r#"SELECT COALESCE(SUM(size_bytes), 0)::bigint AS "used!",
            count(*) FILTER (WHERE context = 'post' AND post_id IS NULL) AS "staged!"
        FROM media WHERE owner_id = $1"#,
        user.id
    )
    .fetch_one(&data.db)
    .await?;
    let limits = Limits {
        image: config.max_image_mb * MB,
        document: config.max_document_mb * MB,
        quota_left: (config.quota_mb * MB).saturating_sub(usage.used.max(0) as u64),
    };

    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|len| len > limits.for_type(None)) {
        return Err(app_error(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_large"));
    }
    if declared.is_some_and(|len| len > limits.quota_left) || limits.quota_left == 0 {
        return Err(app_error(StatusCode::PAYLOAD_TOO_LARGE, "quota_exceeded"));
    }
    if opts.context == MediaContext::Post && usage.staged >= MAX_STAGED {
        return Err(app_error(StatusCode::TOO_MANY_REQUESTS, "too_many_staged"));
    }

    let id = Uuid::new_v4();
    let dir = media::media_dir(&config.dir, id);
    tokio::fs::create_dir_all(&dir).await.map_err(io)?;

    // Store the upload. The dir is removed again on any error
    let stored = async {
        // Avatars are images only
        let classify = |head: &[u8]| {
            detect::detect(head)
                .filter(|d| opts.context == MediaContext::Post || d.media_type == MediaType::Image)
                .ok_or_else(|| app_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format"))
        };

        let path = dir.join(ORIGINAL);
        let mut file = tokio::fs::File::create(&path).await.map_err(io)?;
        let mut stream = body.into_data_stream();
        let mut head: Vec<u8> = Vec::with_capacity(SNIFF_LEN);
        let mut detected: Option<Detected> = None;
        let mut total = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| app_error(StatusCode::BAD_REQUEST, "upload_interrupted"))?;
            total += chunk.len() as u64;
            if total > limits.for_type(detected.as_ref().map(|d| d.media_type)) {
                return Err(app_error(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_large"));
            }
            if total > limits.quota_left {
                return Err(app_error(StatusCode::PAYLOAD_TOO_LARGE, "quota_exceeded"));
            }
            // Reject before taking the whole body
            if detected.is_none() {
                let take = chunk.len().min(SNIFF_LEN - head.len());
                head.extend_from_slice(&chunk[..take]);
                if head.len() == SNIFF_LEN {
                    detected = Some(classify(&head)?);
                }
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        drop(file);

        let detected = match detected {
            Some(d) => d,
            None => classify(&head)?,
        };
        let detected = if detected.mime == "application/zip" {
            let path = path.clone();
            tokio::task::spawn_blocking(move || detect::ooxml_from_zip(&path, detected))
                .await
                .map_err(|e| VialoError::Anyhow(e.into()))?
                .ok_or_else(|| app_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format"))?
        } else {
            detected
        };
        if total > limits.for_type(Some(detected.media_type)) {
            return Err(app_error(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_large"));
        }

        let (status, filename, variants) = match detected.media_type {
            MediaType::Image => {
                let source = detected.source.ok_or_else(|| VialoError::Anyhow(anyhow::anyhow!("image without decoder")))?;
                tokio::task::spawn_blocking(move || process::probe(&path, source))
                    .await
                    .map_err(|e| VialoError::Anyhow(e.into()))?
                    .map_err(|e| match e {
                        ProbeError::Unsupported => app_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format"),
                        ProbeError::Invalid(_) => app_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_image"),
                        ProbeError::TooLarge => app_error(StatusCode::UNPROCESSABLE_ENTITY, "image_too_large"),
                    })?;
                (MediaStatus::Pending, None, Some(vec![]))
            }
            MediaType::Document => {
                // Basename only, no control chars, and the extension must match the content
                let base: String = (opts.filename.as_deref())
                    .and_then(|n| n.rsplit(['/', '\\']).next())
                    .map(|n| n.chars().filter(|c| !c.is_control()).take(200).collect::<String>())
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty() && n != "." && n != "..")
                    .unwrap_or_else(|| id.to_string());
                let suffix = format!(".{}", detected.extension);
                let filename = if base.to_lowercase().ends_with(&suffix) { base } else { base + &suffix };
                (MediaStatus::Ready, Some(filename), None)
            }
        };

        let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
        let mut trans = grab_trans(&mut conn).await?;
        sqlx::query!(
            "INSERT INTO media (id, type, context, mime, filename, size_bytes, owner_id, status, variants)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            id,
            detected.media_type as MediaType,
            opts.context as MediaContext,
            detected.mime,
            filename,
            total as i64,
            user.id,
            status as MediaStatus,
            variants.as_deref()
        )
        .execute(&mut *trans)
        .await?;
        if status == MediaStatus::Pending {
            sqlx::query!(
                "INSERT INTO subsystem_jobs (subsystem, data, created_at, last_updated, status) VALUES ('media', $1, NOW(), NOW(), 'pending')",
                sqlx::types::Json(JobData { media_id: id }) as _
            )
            .execute(&mut *trans)
            .await?;
        }
        trans.commit().await?;

        Ok::<MediaModel, VialoError>(MediaRow {
            id,
            media_type: detected.media_type,
            context: opts.context,
            mime: detected.mime.into(),
            filename,
            size_bytes: total as i64,
            status,
            post_id: None,
            position: 0,
            width: None,
            height: None,
            variants,
        }
        .into())
    }
    .await;

    match stored {
        Ok(model) if model.status == MediaStatus::Pending => {
            data.media_jobs.notify_one();
            Ok((StatusCode::ACCEPTED, Json(model)))
        }
        Ok(model) => Ok((StatusCode::CREATED, Json(model))),
        Err(e) => {
            media::remove_media_dir(&dir).await;
            Err(e)
        }
    }
}

#[utoipa::path(get, path = "/media/{id}", responses((status = 200, description = "OK", body = MediaModel)))]
pub async fn get_media(
    Path(id): Path<Uuid>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let visible = fetch_visible(&data.db, id, user_o.as_ref()).await?;
    Ok(Json(MediaModel::from(visible.row)))
}

#[utoipa::path(get, path = "/media/{id}/{size}",
    params(("size" = u32, Path, description = "Wanted width; the smallest variant at least this wide is served")),
    responses((status = 200, description = "AVIF", content_type = "image/avif", body = Vec<u8>),
        (status = 409, description = "Still encoding or failed")))]
pub async fn serve_image(
    Path((id, size)): Path<(Uuid, u32)>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let Visible { row, public } = fetch_visible(&data.db, id, user_o.as_ref()).await?;
    if row.media_type != MediaType::Image {
        return Err(VialoError::NotFound());
    }
    if row.status != MediaStatus::Ready {
        return Err(app_error(StatusCode::CONFLICT, "media_not_ready"));
    }
    // Smallest variant at least `size` wide, else the largest
    let variants = row.variants.as_deref().unwrap_or_default();
    let width = (variants.iter().copied().filter(|v| *v >= size as i32).min())
        .or_else(|| variants.iter().copied().max())
        .ok_or(VialoError::NotFound())?;
    let path = media::media_dir(&data.config.media.dir, id).join(media::variant_file(width));
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(VialoError::NotFound()),
        Err(e) => return Err(io(e)),
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/avif"),
            (header::CACHE_CONTROL, cache_control(public)),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    ))
}

#[utoipa::path(get, path = "/media/{id}/file",
    responses((status = 200, description = "Document, always as an attachment", content_type = "application/octet-stream", body = Vec<u8>)))]
pub async fn serve_file(
    Path(id): Path<Uuid>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let Visible { row, public } = fetch_visible(&data.db, id, user_o.as_ref()).await?;
    // Image originals keep their EXIF; only variants are served
    if row.media_type != MediaType::Document {
        return Err(VialoError::NotFound());
    }
    let path = media::media_dir(&data.config.media.dir, id).join(ORIGINAL);
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(VialoError::NotFound()),
        Err(e) => return Err(io(e)),
    };
    // RFC 5987 UTF-8 filename; every current browser supports it
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        urlencoding::encode(row.filename.as_deref().unwrap_or_default())
    );
    let headers: [(HeaderName, String); 6] = [
        (header::CONTENT_TYPE, row.mime),
        (header::CONTENT_LENGTH, row.size_bytes.to_string()),
        (header::CONTENT_DISPOSITION, disposition),
        (header::CONTENT_SECURITY_POLICY, "sandbox".into()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        (header::CACHE_CONTROL, cache_control(public).into()),
    ];
    Ok((headers, Body::from_stream(ReaderStream::new(file))))
}

#[utoipa::path(delete, path = "/media/{id}", responses((status = 204, description = "Deleted"), (status = 409, description = "Attached to a post")))]
pub async fn delete_media(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    /*
       PERM
       Owner only.
    */
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;
    let post_id = sqlx::query_scalar!(
        "SELECT post_id FROM media WHERE id = $1 AND owner_id = $2 FOR UPDATE",
        id,
        user.id
    )
    .fetch_optional(&mut *trans)
    .await?
    .ok_or(VialoError::NotFound())?;
    if post_id.is_some() {
        return Err(app_error(StatusCode::CONFLICT, "media_attached"));
    }

    sqlx::query!("DELETE FROM media WHERE id = $1", id)
        .execute(&mut *trans)
        .await?;
    trans.commit().await?;

    media::remove_media_dir(&media::media_dir(&data.config.media.dir, id)).await;
    Ok(StatusCode::NO_CONTENT)
}
