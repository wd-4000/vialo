use serde::{Deserialize, Deserializer, de::Error};
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
