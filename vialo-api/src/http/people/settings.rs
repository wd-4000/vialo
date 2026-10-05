use crate::{
    AppState,
    bookables::BookableEventKind,
    http::util::{
            JsonE, User, VialoError, grab_authd_conn_user,
            grab_trans,
        },
};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sqlx::{query, query_scalar};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct BookableNotificationSettings {
    events: Vec<BookableEventKind>,
    /// Lead time for `upcoming`, kept while it's switched off
    upcoming_minutes: i32,
}

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct NotificationSettings {
    boards: Vec<i32>,
    bookables: BookableNotificationSettings,
}

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct AccountSettings {
    comms_lang: Option<String>,
    notifications: NotificationSettings,
}

#[utoipa::path(get, path = "/people/me/settings", responses((status = 200, body=AccountSettings)))]
pub async fn get_settings(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let person = query!(
        r#"SELECT ap.comms_lang,
        p.events as "events!: Vec<BookableEventKind>",
        p.upcoming_minutes as "upcoming_minutes!"
        FROM accounts_people ap
        JOIN bookable_notification_prefs p ON p.account_id = ap.id
        WHERE ap.id = $1"#,
        user.id
    )
    .fetch_one(&data.db)
    .await?;
    let boards = query_scalar!(
        "SELECT board_id FROM board_subscriptions WHERE account_id = $1",
        user.id
    )
    .fetch_all(&data.db)
    .await?;

    let notifications = NotificationSettings {
        boards,
        bookables: BookableNotificationSettings {
            events: person.events,
            upcoming_minutes: person.upcoming_minutes,
        },
    };
    let account_settings = AccountSettings {
        comms_lang: person.comms_lang,
        notifications,
    };

    Ok(Json(account_settings))
}

#[utoipa::path(put, path = "/people/me/settings", request_body = AccountSettings, responses((status = 204)))]
pub async fn put_settings(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<AccountSettings>,
) -> Result<impl IntoResponse, VialoError> {
    let bookables = &body.notifications.bookables;
    if !(1..=1440).contains(&bookables.upcoming_minutes) {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "upcoming_minutes".into(),
        ));
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;
    query!(
        "UPDATE accounts_people SET comms_lang = $1 WHERE id = $2",
        body.comms_lang,
        user.id
    )
    .execute(&mut *trans)
    .await?;

    query!(
        "INSERT INTO bookable_notification_settings (account_id, events, upcoming_minutes)
        VALUES ($1, $2, $3)
        ON CONFLICT (account_id) DO UPDATE
        SET events = EXCLUDED.events, upcoming_minutes = EXCLUDED.upcoming_minutes",
        user.id,
        &bookables.events as &[BookableEventKind],
        bookables.upcoming_minutes
    )
    .execute(&mut *trans)
    .await?;

    // Very important that this is a diff because subscription ids are unsubscribe tokens 😍
    query!(
        "DELETE FROM board_subscriptions WHERE account_id = $1 AND NOT (board_id = ANY($2))",
        user.id,
        &body.notifications.boards
    )
    .execute(&mut *trans)
    .await?;

    query!(
        "INSERT INTO board_subscriptions (account_id, board_id)
        SELECT $1, b.id FROM boards b
        WHERE b.id = ANY($2) AND account_board_perm_exists($1, b.id, 'view')
        ON CONFLICT (account_id, board_id) DO NOTHING",
        user.id,
        &body.notifications.boards
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}
