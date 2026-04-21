use handlebars::{DirectorySourceOptions, Handlebars};
use icu::locale::locale;
use lettre::{
    Message, SmtpTransport, Transport,
    message::{Mailbox, MultiPart},
};
use serde::Serialize;
use std::{env, sync::Arc};
use yaml_rust2::{Yaml, YamlLoader};
mod custom_headers;
mod date;
use crate::{
    AppState,
    config::{Config, OrgConfig},
};
use custom_headers::*;
use date::get_event_timespan;
use tracing::{debug, error};

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
#[serde(rename_all = "lowercase")]
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

async fn form_email<'a>(
    config: &Config,
    context: EmailContext<'a>,
    locale: Yaml,
    locale_plain: Yaml,
) -> Result<Message, anyhow::Error> {
    let email_domain = config.email_domain();

    // Init templating engine
    let mut handlebars = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars
        .register_templates_directory("src/email/templates", DirectorySourceOptions::default())?;

    for kv in &mut locale["global"].as_hash().unwrap().iter() {
        if let (Yaml::String(k), Yaml::String(v)) = kv {
            handlebars.register_partial(k, v)?;
        }
    }

    match context.message {
        MessageType::Post { .. } => {
            for kv in &mut locale["post"]["new"].as_hash().unwrap().iter() {
                if let (Yaml::String(k), Yaml::String(v)) = kv {
                    handlebars.register_partial(k, v)?;
                }
            }
        }
        MessageType::Direct { .. } => {
            for kv in &mut locale["direct"].as_hash().unwrap().iter() {
                if let (Yaml::String(k), Yaml::String(v)) = kv {
                    handlebars.register_partial(k, v)?;
                }
            }
        }
    }

    let email_title = handlebars.render("title", &context)?;
    let email_content_html = handlebars.render("html/post", &context).unwrap();

    for kv in &mut locale_plain["global"].as_hash().unwrap().iter() {
        if let (Yaml::String(k), Yaml::String(v)) = kv {
            handlebars.register_partial(k, v)?;
        }
    }
    for kv in &mut locale_plain["post"]["new"].as_hash().unwrap().iter() {
        if let (Yaml::String(k), Yaml::String(v)) = kv {
            handlebars.register_partial(k, v)?;
        }
    }

    let email_content_plain = handlebars.render("plain/post", &context).unwrap();

    println!("Testing mail!");
    // Build the email
    let mut builder = Message::builder();

    builder = builder.from(
        (if let Some(group) = context.group {
            format!(
                "{org_name_short} {} <{}@{email_domain}>",
                group.label,
                group.email.as_ref().map_or("noreply", |f| { f.as_str() }),
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

    return Ok(builder
        .to(context.account.email.parse::<Mailbox>()?)
        .subject(email_title)
        .header(custom_headers::ListUnsubscribe(
            format!("<{}>", context.account.url_unsubscribe).into(),
        ))
        .header(ListUnsubscribePost("List-Unsubscribe=One-Click".into()))
        .multipart(MultiPart::alternative_plain_html(
            email_content_plain,
            email_content_html,
        ))?);
}

pub async fn main(app_state: Arc<AppState>) -> Result<(), anyhow::Error> {
    let mailer =
        SmtpTransport::from_url(&env::var("EMAIL_URL").expect("No SMTP URL provided!"))?.build();

    mailer.test_connection()?;
    let locale = YamlLoader::load_from_str(include_str!("langs/en.yaml"))?;
    let locale_plain = YamlLoader::load_from_str(include_str!("langs/en_plain.yaml"))?;

    let post = sqlx::query!(
        r#"SELECT
            bp.id,
            bp.account_id,
            bp.board_id,
            get_i18n_string(b.label, $1) AS "board_label!",
            bp.icon,
            -- Use get_i18n_string for title, content, and location translations with fallback
            get_i18n_string(bp.title, $1) AS title,
            get_i18n_string(bp.location, $1) AS location,
            bp.event_from,
            bp.event_to,
            bp.created_at,
            get_i18n_string(bp.content_html, $1) AS content_html,
            get_i18n_string(bp.content_plain, $1) AS content_plain,
            ag.label as "group_label?",
            ag.email as "group_email"
        FROM
            board_posts bp JOIN boards b ON b.id = bp.board_id LEFT JOIN account_groups ag ON b.group_id = ag.id where bp.id = $2;"#,
        &["en".into(), "de".into()],
        7
    )
    .fetch_one(&app_state.db)
    .await?;

    let url_prefixes = &app_state.config.email.url;

    let message = MessageType::Post {
        board: EmailBoardInfo {
            label: post.board_label,
            url: url_prefixes.board.clone() + &post.board_id.to_string(),
        },
        title: post.title.unwrap_or("No title".into()),
        time: get_event_timespan(locale!("en"), post.event_from, post.event_to),
        url: url_prefixes.post.clone() + &post.id.to_string(),
        content_html: post.content_html,
        content_plain: post.content_plain,
    };

    let people = sqlx::query!(
        r#"SELECT ap.full_name, ap.email as "email!", bs.id as unsubscribe FROM accounts_people ap
        JOIN board_subscriptions bs ON bs.account_id = ap.id
        WHERE ap.email IS NOT NULL AND bs.board_id = $1"#,
        post.board_id
    )
    .fetch_all(&app_state.db)
    .await?;

    for person in people.iter() {
        let email = form_email(
            &app_state.config,
            EmailContext {
                message: &message,
                account: EmailAccountInfo {
                    full_name: &person.full_name.clone().unwrap_or("No name".into()),
                    email: &person.email,
                    url_unsubscribe: url_prefixes.unsubscribe.clone()
                        + &person.unsubscribe.to_string(),
                },
                group: if let Some(group_label) = &post.group_label {
                    Some(EmailGroupInfo {
                        label: group_label,
                        email: &post.group_email,
                    })
                } else {
                    None
                },
                org: app_state.config.org.clone(),
                url_email_preferences: url_prefixes.preferences.clone(),
            },
            locale[0].clone(),
            locale_plain[0].clone(),
        )
        .await?;
        // Send the email
        match mailer.send(&email) {
            Ok(_) => {
                debug!("Post {} notification to {} sent!", post.id, person.email,)
            }
            Err(error) => {
                error!(
                    "Post {} notification for {} failed: {:?}",
                    post.id, person.email, error
                );
            }
        }
    }

    return Ok(());
}
