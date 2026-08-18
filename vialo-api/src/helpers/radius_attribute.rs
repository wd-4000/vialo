use std::ops::Deref;

use serde::{Deserialize, Deserializer, de};
use utoipa::openapi::{
    ArrayBuilder, ObjectBuilder, RefOr, Schema,
    schema::{SchemaType, Type},
};

/// A single RADIUS attribute value as sent by FreeRADIUS rlm_rest.
/// They arrive as `{"type": "...", "value": ["..."]}`, we just preserve value\[0\]
#[derive(Debug)]
pub struct RadiusAttributeValue<T = String>(T);

impl<T> Deref for RadiusAttributeValue<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for RadiusAttributeValue<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Wire shape — unknown fields (e.g. `type`) are ignored.
        #[derive(Deserialize)]
        struct Wire<T> {
            value: Vec<T>,
        }

        let value = Wire::<T>::deserialize(deserializer)?
            .value
            .into_iter()
            .next()
            .ok_or_else(|| de::Error::custom("RADIUS attribute `value` array is empty"))?;

        Ok(RadiusAttributeValue(value))
    }
}

/// Fix schema derivation so the schema describes the wire shape
impl<T: utoipa::PartialSchema> utoipa::__dev::ComposeSchema for RadiusAttributeValue<T> {
    fn compose(mut generics: Vec<RefOr<Schema>>) -> RefOr<Schema> {
        // Empty when the type is used without explicit generic arguments
        let value_schema = if generics.is_empty() {
            T::schema()
        } else {
            generics.remove(0)
        };

        ObjectBuilder::new()
            .property(
                "type",
                ObjectBuilder::new().schema_type(SchemaType::new(Type::String)),
            )
            .property("value", ArrayBuilder::new().items(value_schema).build())
            .required("value")
            .build()
            .into()
    }
}

impl<T: utoipa::PartialSchema> utoipa::ToSchema for RadiusAttributeValue<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        "RadiusAttributeValue".into()
    }
}
