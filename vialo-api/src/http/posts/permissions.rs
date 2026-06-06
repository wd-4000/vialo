use sqlx::PgExecutor;
use uuid::Uuid;

use super::boards::BoardPerm;
use crate::http::util::VialoError;

pub async fn require_board_perm<'c, T: PgExecutor<'c>>(
    user_id: Uuid,
    board_id: i32,
    min_perm: BoardPerm,
    db: T,
) -> Result<(), VialoError> {
    let allowed = sqlx::query_scalar!(
        r#"SELECT account_board_perm_exists($1, $2, $3) AS "allowed: bool""#,
        user_id,
        board_id,
        min_perm as BoardPerm,
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
