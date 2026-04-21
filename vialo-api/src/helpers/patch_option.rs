use serde::{Deserialize, Deserializer, de::Error};
use utoipa::ToSchema;
use utoipa::openapi::{
    KnownFormat, ObjectBuilder, OneOfBuilder, RefOr, Schema, SchemaFormat,
    schema::{SchemaType, Type},
};
use uuid::Uuid;

/** This is, like, an insane wrapper for an Option inside a uhhh Struct. So none = not present, some is either null or a value */
#[derive(Debug, Default)]
pub enum PatchOption<T> {
    /** Field NOT THERE  */
    #[default]
    None,
    /** This might be null if you didn't understand */
    Some(Option<T>),
}

impl<T: utoipa::PartialSchema> utoipa::__dev::ComposeSchema for PatchOption<T> {
    fn compose(mut generics: Vec<RefOr<Schema>>) -> RefOr<Schema> {
        let t_schema = generics.remove(0);
        RefOr::T(Schema::OneOf(
            OneOfBuilder::new()
                .item(t_schema)
                .item(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::Null))
                        .build(),
                )
                .build(),
        ))
    }
}

impl<T: utoipa::ToSchema> utoipa::ToSchema for PatchOption<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        format!("PatchOption_{}", T::name()).into()
    }
}

impl<T> PatchOption<T> {
    pub fn try_map<U, E>(self, f: impl Fn(T) -> Result<U, E>) -> Result<PatchOption<U>, E> {
        match self {
            PatchOption::None => Ok(PatchOption::None),
            PatchOption::Some(None) => Ok(PatchOption::Some(None)),
            PatchOption::Some(Some(v)) => Ok(PatchOption::Some(Some(f(v)?))),
        }
    }
}

impl<T> From<Option<T>> for PatchOption<T> {
    fn from(opt: Option<T>) -> PatchOption<T> {
        match opt {
            Some(v) => PatchOption::Some(Some(v)),
            None => PatchOption::Some(None),
        }
    }
}

impl<'de, T> Deserialize<'de> for PatchOption<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::deserialize(deserializer).map(Into::into)
    }
}
