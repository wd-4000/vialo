use axum::{Json, response::IntoResponse};
use std::collections::BTreeMap;
use utoipa::{
    OpenApi,
    openapi::{extensions::ExtensionsBuilder, tag::TagBuilder},
};
use utoipauto::utoipauto;

/// Fixes these ugly tags. Now.
///
/// Things such as crate::http::bookables -> Bookables,
/// things like crate::http::bookables::schemas -> Bookables/Schemas.
struct FixTheseUglyTagsNow;

impl utoipa::Modify for FixTheseUglyTagsNow {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let mut all_tags: std::collections::BTreeSet<Vec<String>> = Default::default();
        openapi.tags = Some(vec![]);

        for (_, path_item) in openapi.paths.paths.iter_mut() {
            for op in [
                &mut path_item.get,
                &mut path_item.put,
                &mut path_item.post,
                &mut path_item.delete,
                &mut path_item.patch,
            ] {
                if let Some(operation) = op {
                    if let Some(tags) = &mut operation.tags {
                        let new_tags: Vec<Vec<String>> =
                            tags.iter().map(|t| clean_tag(t)).collect();
                        *tags = new_tags.iter().map(|t| t.join("/")).collect();
                        all_tags.extend(new_tags.iter().cloned());
                    }
                }
            }
        }

        // Group tags by root segment for x-tagGroups (used by Redoc)
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for tag_parts in &all_tags {
            let root = tag_parts
                .first()
                .map(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            groups.entry(root).or_default().push(tag_parts.join("/"));

            let extensions_builder = ExtensionsBuilder::new();

            // Trim first element unless that's all we've got
            let trimmed_parts = match tag_parts.as_slice() {
                [] => vec![],
                [single] => vec![single.clone()],
                [_, rest @ ..] => rest.to_vec(),
            };
            let extensions = extensions_builder
                .add("x-displayName", trimmed_parts.join("/"))
                .build();

            let tb = TagBuilder::new();
            openapi.tags.get_or_insert_with(Vec::new).push(
                tb.name(tag_parts.join("/"))
                    .extensions(Some(extensions))
                    .build(),
            );
        }

        let tag_groups: Vec<serde_json::Value> = groups
            .into_iter()
            .map(|(name, tags)| serde_json::json!({"name": name, "tags": tags}))
            .collect();

        let extensions = openapi.extensions.get_or_insert_with(Default::default);
        extensions.insert("x-tagGroups".into(), serde_json::json!(tag_groups));
    }
}

fn clean_tag(tag: &str) -> Vec<String> {
    let stripped = tag.strip_prefix("crate::http::").unwrap_or(tag);
    let parts: Vec<String> = stripped
        .split("::")
        .filter(|&p| !matches!(p, "handlers" | "models" | "mod"))
        .map(to_title_case)
        .collect();
    if parts.is_empty() {
        vec![to_title_case(stripped.split("::").last().unwrap_or(tag)).into()]
    } else {
        parts
    }
}

fn to_title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[utoipauto(paths = "./src/http")]
#[derive(OpenApi)]
#[openapi(
    modifiers(&FixTheseUglyTagsNow),
    info(
        title = "Vialo API",
        version = env!("CARGO_PKG_VERSION"),
        description = "OpenAPI specification generated from the Axum handlers."
    )
)]
pub struct ApiDoc;

pub fn openapi_doc() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

#[utoipa::path(get, path = "/openapi.json", responses((status = 200, description = "OpenAPI document")))]
pub async fn openapi_json() -> impl IntoResponse {
    Json(openapi_doc())
}
