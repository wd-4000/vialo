use crate::{
    helpers::encryption::{self, Encrypted},
    http::{
        network::{mac::MacAddressWrapper, models::NetworkListModel},
        util::models::AccountEmbed,
    },
    impl_jsonb_embed,
};
use serde::{Deserialize, Serialize};
use sqlx::types::ipnetwork::IpNetwork;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct DeviceWithRefs {
    pub id: Uuid,
    pub label: Option<String>,
    pub mobileconfig: bool,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub realm_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct ListDeviceWithRefs {
    pub id: Uuid,
    pub label: Option<String>,
    pub creds: DeviceCredEmbeds,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,
    pub realm_id: Option<Uuid>,
}

/*
*   id uuid PRIMARY KEY default gen_random_uuid(),
username text,
password BYTEA,
account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
network_id int NOT NULL REFERENCES net_networks (id) ON DELETE CASCADE,
last_updated timestamptz
*/
#[derive(Serialize, ToSchema)]
pub struct DeviceWithRefsAndIpAndAccountAndCredentialEmbed {
    pub id: Uuid,
    pub label: Option<String>,
    pub mobileconfig: bool,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub realm_id: Option<Uuid>,

    pub account: AccountEmbed,

    pub creds: DeviceCredEmbedsFull,
    /// Here for deserialization only
    #[serde(skip_serializing)]
    pub cred_passwords: Vec<Option<Encrypted<String>>>,
}

/// One credential attached to a device, with the MAC it is registered under.
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct DeviceCredEmbed {
    pub id: Uuid,
    pub network_id: i32,
    pub mac: Option<MacAddressWrapper>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
#[serde(transparent)]
pub struct DeviceCredEmbeds(pub Vec<DeviceCredEmbed>);

impl_jsonb_embed!(DeviceCredEmbeds);

/// One credential attached to a device, with the MAC it is registered under with the full credential details.
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct DeviceCredEmbedFull {
    pub id: Uuid,
    pub network_id: i32,
    pub username: Option<String>,
    #[schema(value_type = Option<String>)]
    #[serde(serialize_with = "encryption::serialize_exposed_opt")]
    pub password: Option<Encrypted<String>>,
    pub account_id: Uuid,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub network: Option<NetworkListModel>,

    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_addr: Option<IpNetwork>,
    pub mac: Option<MacAddressWrapper>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
#[serde(transparent)]
pub struct DeviceCredEmbedsFull(pub Vec<DeviceCredEmbedFull>);

impl_jsonb_embed!(DeviceCredEmbedsFull);

#[derive(Serialize, ToSchema)]
pub struct DeviceModelWithAccountEmbed {
    pub id: Uuid,
    pub label: Option<String>,
    pub mobileconfig: bool,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub hostname: Option<String>,

    pub account: AccountEmbed,

    pub creds: DeviceCredEmbeds,
    pub realm_id: Option<Uuid>,
}
