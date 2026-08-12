pub mod bookables;
pub mod config;
pub mod dump;
pub mod events;
pub mod health;
pub mod helpers;
pub mod hooks;
pub mod http;
pub mod permissions;
pub mod ws;

#[cfg(feature = "ppsk")]
pub mod ppsk;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "printer")]
pub mod printer;

use crate::{bookables::BookableChannel, config::Config, http::rate_limit::RateLimiters};
use sqlx::{Pool, Postgres};
use tokio::sync::broadcast;

pub struct KratosConfigs {
    pub frontend: ory_kratos_client::apis::configuration::Configuration,
    pub admin: ory_kratos_client::apis::configuration::Configuration,
}

pub struct EventChannels {
    pub bookables: BookableChannel,
    pub posts_tx: broadcast::Sender<i32>,
    pub expired_appointments_tx: broadcast::Sender<crate::bookables::ExpiredAppointmentMessage>,
}

pub struct AppState {
    pub db: Pool<Postgres>,
    pub event_channels: EventChannels,
    pub config: Config,
    pub kratos_config: Option<KratosConfigs>,
    pub rate_limiters: RateLimiters,
}

#[macro_export]
macro_rules! list_i18n_generic {
    ($db:expr, $query:expr, $opts:expr, $result_type:ty) => {{
        use sqlx::query_as;

        let (offset, limit) = $crate::http::util::clamp_pagination($opts.limit, $opts.page)?;
        let langs = $opts
            .lang
            .unwrap_or(vec![String::from("en"), String::from("de")]);
        let record = query_as!($result_type, $query, &langs, limit as i64, offset as i64)
            .fetch_all($db)
            .await?;

        return Ok((StatusCode::OK, Json(record)));
    }};
}
