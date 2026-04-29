use serde::Serialize;
use serde::{Deserialize, Deserializer, de::Error};
use sqlx::{
    Type as SqlxType,
    decode::Decode,
    encode::{Encode, IsNull},
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
};
use utoipa::ToSchema;
use utoipa::openapi::{
    KnownFormat, ObjectBuilder, OneOfBuilder, RefOr, Schema, SchemaFormat,
    schema::{SchemaType, Type},
};
use uuid::Uuid;

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
    Me,
    #[default]
    All,
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

#[derive(Debug, PartialEq)]
pub enum IdOrMeQuery {
    Id(Uuid),
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

impl From<serde_json::Value> for AccountEmbed {
    fn from(value: serde_json::Value) -> Self {
        serde_json::from_value(value).expect("Failed to deserialize AccountEmbed from JSON")
    }
}

impl From<Option<serde_json::Value>> for AccountEmbed {
    fn from(value: Option<serde_json::Value>) -> Self {
        value
            .map(|v| v.into())
            .expect("AccountEmbed column was null but expected non-null")
    }
}

impl SqlxType<sqlx::Postgres> for AccountEmbed {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("JSONB")
    }
}

impl<'r> Decode<'r, sqlx::Postgres> for AccountEmbed {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let json: sqlx::types::Json<Self> = Decode::decode(value)?;
        Ok(json.0)
    }
}

impl<'q> Encode<'q, sqlx::Postgres> for AccountEmbed {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        Encode::encode(sqlx::types::Json(self), buf)
    }
}

impl sqlx::postgres::PgHasArrayType for AccountEmbed {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_JSONB")
    }
}
