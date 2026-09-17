use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;
use sqlx::types::ipnetwork::{self};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    helpers::{I18nMap, PgDate},
    http::util::models::{AccountEmbed, AccountPersonEmbed, ProductEmbed, ProductType, RoomEmbed},
    permissions::{AppRole, GroupRole},
};

#[derive(Serialize, Debug, ToSchema)]
pub struct GroupMemberModel {
    pub id: Uuid,
    pub full_name: Option<String>,
    pub room: Option<RoomEmbed>,
    pub role: GroupRole,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct CurrentRoomOccupantsViewModel {
    pub id: Option<Uuid>,
    pub auth_id: Option<Uuid>,
    pub full_name: Option<String>,
    pub room: Option<RoomEmbed>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PeopleLookupModel {
    pub id: Uuid,
    pub full_name: Option<String>,
    pub room: Option<RoomEmbed>,
}

// id | label  | email  | created_at | public
#[derive(Serialize, Debug, ToSchema)]
pub struct GroupListModel {
    pub id: Uuid,
    pub label: String,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
    pub description: Option<String>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct GroupGetModel {
    pub id: Uuid,
    pub label: String,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
    pub app_roles: Vec<AppRole>,
    pub description: Option<String>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct GroupGetModelAllLanguages {
    pub id: Uuid,
    pub label: String,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
    pub app_roles: Vec<AppRole>,
    #[schema(value_type = Option<I18nMap>)]
    pub description: Option<JsonValue>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct GroupStubListModel {
    pub id: Uuid,
    pub label: String,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct GroupStubListModelWithRole {
    pub id: Uuid,
    pub label: String,
    pub role: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AccountModel {
    pub id: Uuid,
    pub label: Option<String>,
    pub auth_id: Option<Uuid>,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub membership_start: Option<NaiveDate>,
    pub membership_end: Option<PgDate>,
    pub full_name: Option<String>,
    pub manually_suspended: Option<bool>,
    pub credit_balance: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct AccountModelForOverview {
    pub id: Uuid,
    pub label: Option<String>,
    pub auth_id: Option<Uuid>,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub membership_start: Option<NaiveDate>,
    pub membership_end: Option<PgDate>,
    pub full_name: Option<String>,
    pub manually_suspended: Option<bool>,
    pub credit_balance: Option<i32>,
    pub amenities_username: Option<String>,
    #[cfg(feature = "printer")]
    pub printer_username: Option<String>,
    #[cfg(feature = "printer")]
    pub printer_id: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct LeaseModelWithEmbeddedSublease {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub room: Option<RoomEmbed>,
    pub begins: NaiveDate,
    pub lessor: Option<AccountPersonEmbed>,
    pub ends: NaiveDate,
}

#[derive(Serialize)]
pub struct JumboModelTranslated {
    pub id: i32,
    pub img: Option<String>,
    pub headline: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct RoomModel {
    pub id: Uuid,
    pub label: String,
    pub capacity: Option<i32>,
    pub floor: Option<i32>,
    pub vlan: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "transaction_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Done,
    Refunded,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "ledger_entry_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LedgerEntryType {
    Transfer,
    UnlimitedEnabled,
    UnlimitedDisabled,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PersonalTransactionModel {
    pub id: Uuid,
    pub to_account: Option<AccountEmbed>,
    pub from_account: Option<AccountEmbed>,
    pub product: Option<ProductType>,
    pub credits: Option<i32>,
    pub status: TransactionStatus,
    pub refund_of: Option<Uuid>,
    pub entry_type: LedgerEntryType,
    pub label: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PersonalTransactionModelWithProductDetails {
    pub id: Uuid,
    pub to_account: Option<AccountEmbed>,
    pub from_account: Option<AccountEmbed>,
    pub product: Option<ProductEmbed>,
    pub label: Option<String>,
    pub credits: Option<i32>,
    pub status: TransactionStatus,
    pub refund_of: Option<Uuid>,
    pub entry_type: LedgerEntryType,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PersonalNetRealmOverviewModel {
    pub id: Uuid,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_nat: Option<ipnetwork::IpNetwork>,
}

#[derive(Serialize, ToSchema)]
pub struct PersonOverviewNetworkModel {
    pub realms: Vec<PersonalNetRealmOverviewModel>,
    pub devices: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PersonOverviewModel {
    #[serde(flatten)]
    pub account: AccountModelForOverview,
    pub groups: Vec<GroupStubListModelWithRole>,
    pub contract: Option<LeaseModelWithEmbeddedSublease>,
    pub transactions: Vec<PersonalTransactionModel>,
    pub network: PersonOverviewNetworkModel,
}

#[derive(Serialize, ToSchema)]
pub struct CreatePersonNetworkRealm {
    pub id: Uuid,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_nat: Option<ipnetwork::IpNetwork>,
}

#[derive(Serialize, ToSchema)]
pub struct CreatePersonNetwork {
    pub realm: CreatePersonNetworkRealm,
}

#[derive(Serialize, ToSchema)]
pub struct CreatePersonResponse {
    pub id: Uuid,
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<CreatePersonNetwork>,
}
