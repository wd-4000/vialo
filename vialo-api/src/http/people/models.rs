use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{
    FromRow,
    types::{
        JsonValue,
        ipnetwork::{self},
    },
};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    helpers::PgDate,
    http::util::models::AccountEmbed,
    permissions::{AppRole, GroupRole},
};

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct GroupMemberModel {
    pub id: Option<Uuid>,
    pub full_name: Option<String>,
    pub room_name: Option<String>,
    pub room_id: Option<Uuid>,
    pub role: GroupRole,
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct CurrentRoomOccupantsViewModel {
    pub id: Option<Uuid>,
    pub auth_id: Option<Uuid>,
    pub full_name: Option<String>,
    pub room_name: Option<String>,
    pub room_id: Option<Uuid>,
}

// id | label  | email  | created_at | public
#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct GroupListModel {
    pub id: Uuid,
    pub label: String,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct GroupGetModel {
    pub id: Uuid,
    pub label: String,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
    pub app_roles: Vec<AppRole>,
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct GroupStubListModel {
    pub id: Uuid,
    pub label: String,
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct GroupStubListModelWithRole {
    pub id: Uuid,
    pub label: String,
    pub role: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AccountModel {
    pub id: Option<Uuid>,
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
    pub id: Option<Uuid>,
    pub label: Option<String>,
    pub auth_id: Option<Uuid>,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub membership_start: Option<NaiveDate>,
    pub membership_end: Option<PgDate>,
    pub full_name: Option<String>,
    pub manually_suspended: Option<bool>,
    pub credit_balance: Option<i32>,
    #[cfg(feature = "printer")]
    pub printer_username: Option<String>,
    #[cfg(feature = "printer")]
    pub printer_id: Option<i32>,
}

#[derive(Deserialize, Serialize, Debug, ToSchema)]
pub struct LessorModel {
    pub id: Uuid,
    pub name: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LeaseModelWithEmbeddedSublease {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub room_id: Uuid,
    pub room: String,
    pub begins: NaiveDate,
    pub lessor: Option<JsonValue>,
    pub ends: NaiveDate,
}

#[derive(Serialize)]
pub struct JumboModelTranslated {
    pub id: Option<i32>,
    pub img: Option<String>,
    pub headline: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct RoomModel {
    pub id: Option<Uuid>,
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
#[sqlx(type_name = "product_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProductType {
    PrinterColor,
    PrinterBw,
    Bookable,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PersonalTransactionModel {
    pub id: Option<Uuid>,
    pub to_account: Option<AccountEmbed>,
    pub from_account: Option<AccountEmbed>,
    pub product: Option<ProductType>,
    pub credits: Option<i32>,
    pub status: TransactionStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Debug, ToSchema)]
pub struct PersonalTransactionModelWithProductDetails {
    pub id: Option<Uuid>,
    pub to_account: Option<AccountEmbed>,
    pub from_account: Option<AccountEmbed>,
    pub product: Option<JsonValue>,
    pub label: Option<String>,
    pub credits: Option<i32>,
    pub status: TransactionStatus,
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
    pub groups: Vec<GroupStubListModel>,
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
