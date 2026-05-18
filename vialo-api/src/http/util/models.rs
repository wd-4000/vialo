use serde::Serialize;
use serde::{Deserialize, Deserializer, de::Error};
use utoipa::ToSchema;
use utoipa::openapi::{
    KnownFormat, ObjectBuilder, OneOfBuilder, RefOr, Schema, SchemaFormat,
    schema::{SchemaType, Type},
};
use uuid::Uuid;

#[macro_export]
macro_rules! impl_jsonb_embed {
    ($t:ty) => {
        impl From<serde_json::Value> for $t {
            fn from(value: serde_json::Value) -> Self {
                serde_json::from_value(value).expect(concat!(
                    "Failed to deserialize ",
                    stringify!($t),
                    " from JSON"
                ))
            }
        }

        impl From<Option<serde_json::Value>> for $t {
            fn from(value: Option<serde_json::Value>) -> Self {
                value.map(|v| v.into()).expect(concat!(
                    stringify!($t),
                    " column was null but expected non-null"
                ))
            }
        }

        impl sqlx::Type<sqlx::Postgres> for $t {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                sqlx::postgres::PgTypeInfo::with_name("JSONB")
            }
        }

        impl<'r> sqlx::decode::Decode<'r, sqlx::Postgres> for $t {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let json: sqlx::types::Json<Self> = sqlx::decode::Decode::decode(value)?;
                Ok(json.0)
            }
        }

        impl<'q> sqlx::encode::Encode<'q, sqlx::Postgres> for $t {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                sqlx::encode::Encode::encode(sqlx::types::Json(self), buf)
            }
        }

        impl sqlx::postgres::PgHasArrayType for $t {
            fn array_type_info() -> sqlx::postgres::PgTypeInfo {
                sqlx::postgres::PgTypeInfo::with_name("_JSONB")
            }
        }
    };
}

#[derive(Debug, PartialEq)]
pub enum IdOrAllQuery {
    Id(Uuid),
    All,
}

impl utoipa::PartialSchema for IdOrAllQuery {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::OneOf(
            OneOfBuilder::new()
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                        .build(),
                )
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .enum_values(Some(["all"]))
                        .build(),
                )
                .build(),
        ))
    }
}

impl utoipa::ToSchema for IdOrAllQuery {
    fn name() -> std::borrow::Cow<'static, str> {
        "IdOrAllQuery".into()
    }
}

impl<'de> Deserialize<'de> for IdOrAllQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "all" => IdOrAllQuery::All,
            id => IdOrAllQuery::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

#[derive(Debug, PartialEq, Default)]
pub enum IdOrMeOrAllQuery {
    Id(Uuid),
    #[default]
    Me,
    All,
}
impl IdOrMeOrAllQuery {
    /// Converts "me" into the actual Uuid, or returns the specific ID/All.
    pub fn resolve(&self, current_user_id: Uuid) -> IdOrAllQuery {
        match self {
            Self::Id(id) => IdOrAllQuery::Id(*id),
            Self::Me => IdOrAllQuery::Id(current_user_id),
            Self::All => IdOrAllQuery::All,
        }
    }
}

impl utoipa::PartialSchema for IdOrMeOrAllQuery {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::OneOf(
            OneOfBuilder::new()
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                        .build(),
                )
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .enum_values(Some(["me", "all"]))
                        .build(),
                )
                .build(),
        ))
    }
}

impl utoipa::ToSchema for IdOrMeOrAllQuery {
    fn name() -> std::borrow::Cow<'static, str> {
        "IdOrMeOrAllQuery".into()
    }
}

impl<'de> Deserialize<'de> for IdOrMeOrAllQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "all" => Self::All,
            "me" => Self::Me,
            id => Self::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

#[derive(Debug, PartialEq, Default)]
pub enum IdOrMeQuery {
    Id(Uuid),
    #[default]
    Me,
}

impl utoipa::PartialSchema for IdOrMeQuery {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::OneOf(
            OneOfBuilder::new()
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                        .build(),
                )
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .enum_values(Some(["me"]))
                        .build(),
                )
                .build(),
        ))
    }
}

impl utoipa::ToSchema for IdOrMeQuery {
    fn name() -> std::borrow::Cow<'static, str> {
        "IdOrMeQuery".into()
    }
}

impl<'de> Deserialize<'de> for IdOrMeQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "me" => Self::Me,
            id => Self::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    Person,
    Group,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
pub struct AccountEmbed {
    pub id: Uuid,
    pub full_name: String,
    #[serde(rename = "type")]
    pub account_type: AccountType,
}

impl_jsonb_embed!(AccountEmbed);

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "product_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProductType {
    PrinterColor,
    PrinterBw,
    Bookable,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
pub struct ProductEmbed {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub product_type: ProductType,
}

impl_jsonb_embed!(ProductEmbed);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
pub struct AccountPersonEmbed {
    pub id: Uuid,
    pub full_name: String,
    pub room: Option<RoomEmbed>,
}

impl_jsonb_embed!(AccountPersonEmbed);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
pub struct RoomEmbed {
    pub id: Uuid,
    pub label: String,
}

impl_jsonb_embed!(RoomEmbed);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
pub struct GroupEmbed {
    pub id: Uuid,
    pub label: String,
}

impl_jsonb_embed!(GroupEmbed);
