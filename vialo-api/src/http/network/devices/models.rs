use crate::{
    helpers::encryption::{self, Encrypted},
    http::{
        network::{mac::MacAddressWrapper, models::NetworkListModel},
        util::models::AccountEmbed,
    },
    impl_jsonb_embed,
};
use serde::{Deserialize, Serialize};
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
pub struct ListDeviceWithRefs {
    pub id: Uuid,
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,
    pub network_id: Option<i32>,
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

#[derive(Serialize, Debug, Deserialize, ToSchema)]
/*
*   id uuid PRIMARY KEY default gen_random_uuid(),
username text,
password BYTEA,
account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
network_id int NOT NULL REFERENCES net_networks (id) ON DELETE CASCADE,
last_updated timestamptz
*/
pub struct NetworkCredentialEmbed {
    pub id: Uuid,
    pub username: Option<String>,
    #[schema(value_type = Option<String>)]
    #[serde(serialize_with = "encryption::serialize_exposed_opt")]
    pub password: Option<Encrypted<String>>,
    pub account_id: Uuid,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub network: Option<NetworkListModel>,
}

impl_jsonb_embed!(NetworkCredentialEmbed);

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

    pub account: AccountEmbed,

    pub cred: Option<NetworkCredentialEmbed>,
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
    pub network_id: Option<i32>,

    pub account: AccountEmbed,

    pub cred_id: Option<Uuid>,
    pub realm_id: Option<Uuid>,
}
