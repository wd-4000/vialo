use std::error::Error;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::openapi::{ObjectBuilder, OneOfBuilder, RefOr, Schema, SchemaFormat, KnownFormat, schema::{SchemaType, Type}};
use sqlx::{
    Encode,
    encode::IsNull,
    postgres::{PgArgumentBuffer, PgTypeInfo, types::Oid},
};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PgDateTime {
    Infinity,
    NegativeInfinity,
    DateTime(DateTime<Utc>),
}

impl utoipa::PartialSchema for PgDateTime {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::OneOf(
            OneOfBuilder::new()
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .enum_values(Some(["infinity", "negative_infinity"]))
                        .build(),
                )
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::Object))
                        .property(
                            "date_time",
                            ObjectBuilder::new()
                                .schema_type(SchemaType::new(Type::String))
                                .format(Some(SchemaFormat::KnownFormat(KnownFormat::DateTime)))
                                .build(),
                        )
                        .required("date_time")
                        .build(),
                )
                .build(),
        ))
    }
}

impl utoipa::ToSchema for PgDateTime {
    fn name() -> std::borrow::Cow<'static, str> {
        "PgDateTime".into()
    }
}

impl sqlx::Type<sqlx::Postgres> for PgDateTime {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <DateTime<Utc> as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl sqlx::postgres::PgHasArrayType for PgDateTime {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        //PgType::TimestamptzArray
        PgTypeInfo::with_oid(Oid(1182))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgDateTime {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        match value.format() {
            sqlx::postgres::PgValueFormat::Text => match value.as_str()? {
                "infinity" => Ok(PgDateTime::Infinity),
                "-infinity" => Ok(PgDateTime::NegativeInfinity),
                _ => DateTime::<Utc>::decode(value).map(PgDateTime::DateTime),
            },
            sqlx::postgres::PgValueFormat::Binary => {
                let bytes = value.as_bytes()?;
                match bytes {
                    b"\x7F\xFF\xFF\xFF" | b"\x7F\xFF\xFF\xFF\xFF\xFF\xFF\xFF" => {
                        Ok(PgDateTime::Infinity)
                    }
                    b"\x80\x00\x00\x00" | b"\x80\x00\x00\x00\x00\x00\x00\x00" => {
                        Ok(PgDateTime::NegativeInfinity)
                    }
                    _ => DateTime::<Utc>::decode(value).map(PgDateTime::DateTime),
                }
            }
        }
    }
}

impl<'r> Encode<'r, sqlx::Postgres> for PgDateTime {
    #[allow(unused)]
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<IsNull, Box<dyn Error + Send + Sync + 'static>> {
        match self {
            PgDateTime::Infinity => {
                buf.extend(b"\x7F\xFF\xFF\xFF\xFF\xFF\xFF\xFF");
            }
            PgDateTime::NegativeInfinity => {
                buf.extend(b"\x80\x00\x00\x00\x00\x00\x00\x00");
            }
            PgDateTime::DateTime(dt) => {
                // Delegate the encoding to the DateTime<Utc> type and handle potential errors
                <DateTime<Utc> as Encode<sqlx::Postgres>>::encode_by_ref(dt, buf)?;
            }
        }
        Ok(IsNull::No)
    }
}
