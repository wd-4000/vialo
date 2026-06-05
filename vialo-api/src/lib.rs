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

use crate::{bookables::BookableChannel, config::Config};
use sqlx::{Pool, Postgres};

pub struct KratosConfigs {
    pub frontend: ory_kratos_client::apis::configuration::Configuration,
    pub admin: ory_kratos_client::apis::configuration::Configuration,
}

pub struct EventChannels {
    pub bookables: BookableChannel,
}

pub struct AppState {
    pub db: Pool<Postgres>,
    pub event_channels: EventChannels,
    pub config: Config,
    pub kratos_config: Option<KratosConfigs>,
}

#[macro_export]
macro_rules! list_i18n_generic {
    ($db:expr, $query:expr, $opts:expr, $result_type:ty) => {{
        use sqlx::query_as;

        let limit = $opts.limit.unwrap_or(10);
        let langs = $opts
            .lang
            .unwrap_or(vec![String::from("en"), String::from("de")]);
        let offset = ($opts.page.unwrap_or(1) - 1) * limit;
        let record = query_as!($result_type, $query, &langs, limit as i32, offset as i32)
            .fetch_all($db)
            .await?;

        return Ok((StatusCode::OK, Json(record)));
    }};
}
