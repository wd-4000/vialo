use handlebars::{DirectorySourceOptions, Handlebars};
use icu::locale::locale;
use lettre::{
    Message, SmtpTransport, Transport,
    message::{Mailbox, MultiPart},
};
use serde::Serialize;
use std::{env, sync::Arc};
use tokio::sync::{broadcast::error::RecvError, watch};
use yaml_rust2::{Yaml, YamlLoader};
mod custom_headers;
mod date;
use crate::{
    AppState,
    config::{Config, OrgConfig},
};
use custom_headers::*;
use date::get_event_timespan;
use tracing::{debug, error, warn};

#[derive(Serialize)]
struct EmailGroupInfo<'a> {
    pub label: &'a str,
    pub email: &'a Option<String>,
}

#[derive(Serialize)]
struct EmailBoardInfo {
    pub label: String,
    pub url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum MessageType {
    Post {
        board: EmailBoardInfo,
        title: String,
        time: Option<String>,
        content_html: Option<String>,
        content_plain: Option<String>,
        url: String,
    },
    Direct {
        subject: String,
        body: String,
    },
    BookableExpired {
        asset_name: String,
        credits_refunded: i32,
    },
}

#[derive(Serialize)]
struct EmailAccountInfo<'a> {
    full_name: &'a str,
    email: &'a str,
    url_unsubscribe: String,
}

#[derive(Serialize)]
struct EmailContext<'a> {
    pub message: &'a MessageType,
    pub account: EmailAccountInfo<'a>,
    pub org: OrgConfig,
    pub group: Option<EmailGroupInfo<'a>>,
    pub url_email_preferences: String,
}

fn register_locale_partials(handlebars: &mut Handlebars, yaml: &Yaml) -> Result<(), anyhow::Error> {
    let hash = yaml
        .as_hash()
        .ok_or_else(|| anyhow::anyhow!("expected YAML hash for locale section"))?;
    for (k, v) in hash.iter() {
        if let (Yaml::String(k), Yaml::String(v)) = (k, v) {
            handlebars.register_partial(k, v)?;
        }
    }
    Ok(())
}

async fn form_email<'a>(
    config: &Config,
    context: EmailContext<'a>,
    locale: Yaml,
    locale_plain: Yaml,
) -> Result<Message, anyhow::Error> {
    let email_domain = config.email_domain();

    let (html_template, plain_template, html_msg_locale, plain_msg_locale) = match context.message {
        MessageType::Post { .. } => (
            "html/post",
            "plain/post",
            locale["post"]["new"].clone(),
            locale_plain["post"]["new"].clone(),
        ),
        MessageType::Direct { .. } => (
            "html/direct",
            "plain/direct",
            locale["direct"].clone(),
            locale_plain["direct"].clone(),
        ),
        MessageType::BookableExpired { .. } => (
            "html/bookable_expired",
            "plain/bookable_expired",
            locale["bookable"]["expired"].clone(),
            locale_plain["bookable"]["expired"].clone(),
        ),
    };

    let mut handlebars = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars
        .register_templates_directory("src/email/templates", DirectorySourceOptions::default())?;

    register_locale_partials(&mut handlebars, &locale["global"])?;
    register_locale_partials(&mut handlebars, &html_msg_locale)?;

    let email_title = handlebars.render("title", &context)?;
    let email_content_html = handlebars.render(html_template, &context)?;

    register_locale_partials(&mut handlebars, &locale_plain["global"])?;
    register_locale_partials(&mut handlebars, &plain_msg_locale)?;

    let email_content_plain = handlebars.render(plain_template, &context)?;

    let mut builder = Message::builder();

    builder = builder.from(
        (if let Some(group) = context.group {
            format!(
                "{org_name_short} {} <{}@{email_domain}>",
                group.label,
                group.email.as_ref().map_or("noreply", |f| f.as_str()),
                org_name_short = context.org.short_name,
            )
        } else {
            format!(
                "{org_name_short} <noreply@{email_domain}>",
                org_name_short = context.org.short_name,
            )
        })
        .parse::<Mailbox>()?,
    );

    Ok(builder
        .to(context.account.email.parse::<Mailbox>()?)
        .subject(email_title)
        .header(custom_headers::ListUnsubscribe(format!(
            "<{}>",
            context.account.url_unsubscribe
        )))
        .header(ListUnsubscribePost("List-Unsubscribe=One-Click".into()))
        .multipart(MultiPart::alternative_plain_html(
            email_content_plain,
            email_content_html,
        ))?)
}

async fn notify_post_subscribers(
    app_state: &AppState,
    mailer: &SmtpTransport,
    post_id: i32,
    locale: &Yaml,
    locale_plain: &Yaml,
) -> Result<(), anyhow::Error> {
    let post = sqlx::query!(
        r#"SELECT
            bp.id,
            bp.board_id,
            get_i18n_string(b.label, $1) AS "board_label!",
            get_i18n_string(bp.title, $1) AS title,
            bp.event_from,
            bp.event_to,
            get_i18n_string(bp.content_html, $1) AS content_html,
            get_i18n_string(bp.content_plain, $1) AS content_plain,
            ag.label as "group_label?",
            ag.email as "group_email"
        FROM board_posts bp
        JOIN boards b ON b.id = bp.board_id
        LEFT JOIN account_groups ag ON b.group_id = ag.id
        WHERE bp.id = $2"#,
        &["en".into(), "de".into()],
        post_id
    )
    .fetch_one(&app_state.db)
    .await?;

    let url_prefixes = &app_state.config.email.url;

    let message = MessageType::Post {
        board: EmailBoardInfo {
            label: post.board_label,
            url: url_prefixes.board.clone() + &post.board_id.to_string(),
        },
        title: post.title.unwrap_or_else(|| "No title".into()),
        time: get_event_timespan(locale!("en"), post.event_from, post.event_to),
        url: url_prefixes.post.clone() + &post.id.to_string(),
        content_html: post.content_html,
        content_plain: post.content_plain,
    };

    let people = sqlx::query!(
        r#"SELECT ap.full_name, ap.email as "email!", bs.id as unsubscribe
        FROM accounts_people ap
        JOIN board_subscriptions bs ON bs.account_id = ap.id
        WHERE ap.email IS NOT NULL AND bs.board_id = $1"#,
        post.board_id
    )
    .fetch_all(&app_state.db)
    .await?;

    for person in people.iter() {
        let full_name = person.full_name.as_deref().unwrap_or("");
        let email = form_email(
            &app_state.config,
            EmailContext {
                message: &message,
                account: EmailAccountInfo {
                    full_name,
                    email: &person.email,
                    url_unsubscribe: url_prefixes.unsubscribe.clone()
                        + &person.unsubscribe.to_string(),
                },
                group: post.group_label.as_ref().map(|label| EmailGroupInfo {
                    label,
                    email: &post.group_email,
                }),
                org: app_state.config.org.clone(),
                url_email_preferences: url_prefixes.preferences.clone(),
            },
            locale.clone(),
            locale_plain.clone(),
        )
        .await?;

        match mailer.send(&email) {
            Ok(_) => debug!("Post {} notification to {} sent!", post_id, person.email),
            Err(e) => {
                error!("Post {} notification for {} failed: {:?}", post_id, person.email, e);
                crate::health::add_health_event(
                    &app_state.db,
                    crate::http::history::models::Subsystem::Email,
                    "smtp_failure",
                    Some(serde_json::json!({"post_id": post_id, "recipient": person.email, "error": e.to_string()})),
                    5,
                    false,
                )
                .await;
            }
        }
    }

    Ok(())
}

async fn notify_expired_appointment(
    app_state: &AppState,
    mailer: &SmtpTransport,
    message: &crate::bookables::ExpiredAppointmentMessage,
    locale: &Yaml,
    locale_plain: &Yaml,
) -> Result<(), anyhow::Error> {
    let url_prefixes = &app_state.config.email.url;
    let full_name = message.full_name.as_deref().unwrap_or("");

    let message_type = MessageType::BookableExpired {
        asset_name: message.asset_name.clone(),
        credits_refunded: message.credits_refunded,
    };

    let email = form_email(
        &app_state.config,
        EmailContext {
            message: &message_type,
            account: EmailAccountInfo {
                full_name,
                email: &message.email,
                url_unsubscribe: url_prefixes.preferences.clone(),
            },
            group: None,
            org: app_state.config.org.clone(),
            url_email_preferences: url_prefixes.preferences.clone(),
        },
        locale.clone(),
        locale_plain.clone(),
    )
    .await?;

    match mailer.send(&email) {
        Ok(_) => debug!(
            "Expired appointment {} notification to {} sent!",
            message.id, message.email
        ),
        Err(e) => {
            error!("Expired appointment {} notification for {} failed: {:?}", message.id, message.email, e);
            crate::health::add_health_event(
                &app_state.db,
                crate::http::history::models::Subsystem::Email,
                "smtp_failure",
                Some(serde_json::json!({"appointment_id": message.id, "recipient": message.email, "error": e.to_string()})),
                20,
                false,
            )
            .await;
        }
    }

    Ok(())
}

pub async fn main(
    app_state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), anyhow::Error> {
    let mailer =
        SmtpTransport::from_url(&env::var("EMAIL_URL").expect("No SMTP URL provided!"))?.build();

    mailer.test_connection()?;

    let locale = YamlLoader::load_from_str(include_str!("langs/en.yaml"))?;
    let locale_plain = YamlLoader::load_from_str(include_str!("langs/en_plain.yaml"))?;
    let locale = locale
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty locale file"))?;
    let locale_plain = locale_plain
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty locale_plain file"))?;

    let mut posts_rx = app_state.event_channels.posts_tx.subscribe();
    let mut expired_rx = app_state.event_channels.expired_appointments_tx.subscribe();

    loop {
        tokio::select! {
            result = posts_rx.recv() => {
                match result {
                    Ok(post_id) => {
                        if let Err(e) = notify_post_subscribers(
                            &app_state, &mailer, post_id, &locale, &locale_plain,
                        ).await {
                            error!("Post {} notification failed: {:?}", post_id, e);
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("Email subsystem lagged, missed {} post notifications", n);
                        crate::health::add_health_event(
                            &app_state.db,
                            crate::http::history::models::Subsystem::Email,
                            "channel_lagged",
                            Some(serde_json::json!({"missed": n, "channel": "posts"})),
                            10,
                            false,
                        )
                        .await;
                    }
                    Err(RecvError::Closed) => return Ok(()),
                }
            }
            result = expired_rx.recv() => {
                match result {
                    Ok(message) => {
                        if let Err(e) = notify_expired_appointment(
                            &app_state, &mailer, &message, &locale, &locale_plain,
                        ).await {
                            error!(
                                "Expired appointment {} notification failed: {e:?}",
                                message.id
                            );
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("Email subsystem lagged, missed {} expiry notifications", n);
                        crate::health::add_health_event(
                            &app_state.db,
                            crate::http::history::models::Subsystem::Email,
                            "channel_lagged",
                            Some(serde_json::json!({"missed": n, "channel": "expired_appointments"})),
                            30,
                            false,
                        )
                        .await;
                    }
                    Err(RecvError::Closed) => return Ok(()),
                }
            }
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
        }
    }

    Ok(())
}
