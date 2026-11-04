use reqwest::StatusCode;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{http::util::VialoError, media::models::MediaContext};

/// Makes `media` and the cover, if not in there, the post's media in that order. Staged uploads
/// must belong to `user_id`, the cover must be an image.
/// Deletes the rows of dropped media and returns their ids, so their folders can go after commit.
/// Locks the rows, so call inside the post's transaction.
pub async fn sync_post_media(
    conn: &mut PgConnection,
    user_id: Uuid,
    post_id: i32,
    media: &[Uuid],
    cover_id: Option<Uuid>,
) -> Result<Vec<Uuid>, VialoError> {
    let invalid = || VialoError::AppError(StatusCode::BAD_REQUEST, "invalid_media".into());

    let mut ids = media.to_vec();
    // Usually isn't in the content
    if let Some(cover) = cover_id.filter(|c| !ids.contains(c)) {
        ids.push(cover);
    }
    if ids.len() > 20 || ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
        return Err(invalid());
    }

    let rows = sqlx::query!(
        r#"SELECT id, context AS "context: MediaContext", owner_id, post_id,
            type = 'image' AS "is_image!"
        FROM media WHERE post_id = $1 OR id = ANY($2) ORDER BY id FOR UPDATE"#,
        post_id,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;

    // Already on this post, or staged by the editor
    for id in &ids {
        let row = rows.iter().find(|r| r.id == *id).ok_or_else(invalid)?;
        let ok = match row.post_id {
            Some(p) => p == post_id,
            None => row.context == MediaContext::Post && row.owner_id == user_id,
        };
        if !ok || (cover_id == Some(*id) && !row.is_image) {
            return Err(invalid());
        }
    }

    let dropped: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.post_id == Some(post_id) && !ids.contains(&r.id))
        .map(|r| r.id)
        .collect();
    sqlx::query!("DELETE FROM media WHERE id = ANY($1)", &dropped)
        .execute(&mut *conn)
        .await?;

    sqlx::query!(
        "UPDATE media m SET post_id = $1, position = v.pos
        FROM unnest($2::uuid[]) WITH ORDINALITY AS v(id, pos) WHERE m.id = v.id",
        post_id,
        &ids
    )
    .execute(&mut *conn)
    .await?;

    Ok(dropped)
}
