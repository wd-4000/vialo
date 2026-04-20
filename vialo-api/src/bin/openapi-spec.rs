/**
 * Exports the OpenAPI specification to a JSON file.
 */
fn main() {
    let output_path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("Usage: openapi-spec <output_path>");

    let json = serde_json::to_vec_pretty(&vialo_api::http::docs::openapi_doc())
        .expect("Failed to serialize OpenAPI");
    std::fs::write(output_path, json).expect("Failed to write OpenAPI file");
}
