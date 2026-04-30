use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// A map of language codes to translated strings.
///
/// Represents internationalized content where keys are ISO language codes (e.g., "en", "de").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ToSchema)]
#[schema(
    value_type = HashMap<String, String>,
    example = json!({"en": "something", "de": "etwas"})
)]
pub struct I18nMap(pub HashMap<String, String>);

impl From<I18nMap> for HashMap<String, String> {
    fn from(m: I18nMap) -> Self {
        m.0
    }
}
