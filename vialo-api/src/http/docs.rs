use axum::{Json, response::IntoResponse};
use utoipa::OpenApi;
use utoipauto::utoipauto;

/// Fixes these ugly tags. Now.
///
/// Things such as crate::http::bookables -> Bookables,
/// things like crate::http::bookables::schemas -> Bookables/Schemas.
struct FixTheseUglyTagsNow;

impl utoipa::Modify for FixTheseUglyTagsNow {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
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
                        *tags = tags.iter().map(|t| clean_tag(t)).collect();
                    }
                }
            }
        }
    }
}

fn clean_tag(tag: &str) -> String {
    let stripped = tag.strip_prefix("crate::http::").unwrap_or(tag);
    let parts: Vec<String> = stripped
        .split("::")
        .filter(|&p| !matches!(p, "handlers" | "models" | "mod"))
        .map(to_title_case)
        .collect();
    if parts.is_empty() {
        to_title_case(stripped.split("::").last().unwrap_or(tag))
    } else {
        parts.join("/")
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
