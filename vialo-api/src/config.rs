use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct PublicApiConfig {
    pub cors_origins: Vec<String>,
    pub listen: String,
}

#[derive(Deserialize)]
pub struct HooksApiConfig {
    pub listen: String,
}

#[cfg(feature = "email")]
#[derive(Deserialize)]
pub struct EmailApiUrlConfig {
    pub unsubscribe: String,
    pub post: String,
    pub board: String,
    pub preferences: String,
}

#[cfg(feature = "email")]
#[derive(Deserialize)]
pub struct EmailApiConfig {
    pub url: EmailApiUrlConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AuthConfig {
    Mock {
        uuid: Uuid,
        email: String,
    },
    Kratos {
        frontend_url: String,
        admin_url: String,
    },
}

#[derive(Deserialize)]
pub struct Config {
    pub proxy: Option<String>,
    pub public: PublicApiConfig,
    pub hooks: HooksApiConfig,
    pub auth: AuthConfig,
    #[cfg(feature = "email")]
    pub email: EmailApiConfig,
}
