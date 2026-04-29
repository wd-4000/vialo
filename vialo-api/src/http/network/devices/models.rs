use crate::http::network::mac::MacAddressWrapper;
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, types::ipnetwork::IpNetwork};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceBasic {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceWithRefs {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceWithRefsAndIp {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_addr: Option<IpNetwork>,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceWithRefsAndIpAndAccountAndCredentialEmbed {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,

    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_addr: Option<IpNetwork>,

    pub account: Option<Value>, // TODO Make not optional

    pub cred: Option<Value>,
    pub network: Option<Value>,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceModelWithAccount {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub account_id: Option<Uuid>, // TODO Make not optional
    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct DeviceModelWithAccountEmbed {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub account: Option<Value>, // TODO Make not optional
    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,
}
