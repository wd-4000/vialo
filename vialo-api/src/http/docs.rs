use axum::{Json, response::IntoResponse};
use utoipa::OpenApi;
use utoipauto::utoipauto;

#[utoipauto(paths = "./src/http")]
#[derive(OpenApi)]
#[openapi(
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
