/**
 * Exports the OpenAPI specifications to JSON files.
 *
 * The public API and the hooks API are served on separate listeners and get
 * separate documents — only the public one feeds the UIs' generated types.
 */
fn main() {
    let mut args = std::env::args().skip(1);
    let (public_path, hooks_path) = args
        .next()
        .zip(args.next())
        .map(|(p, h)| (std::path::PathBuf::from(p), std::path::PathBuf::from(h)))
        .expect("Usage: openapi-spec <public_output_path> <hooks_output_path>");

    for (path, doc) in [
        (public_path, vialo_api::http::docs::openapi_doc()),
        (hooks_path, vialo_api::http::docs::hooks_openapi_doc()),
    ] {
        let json = serde_json::to_vec_pretty(&doc).expect("Failed to serialize OpenAPI");
        std::fs::write(&path, json)
            .unwrap_or_else(|err| panic!("Failed to write OpenAPI file {}: {err}", path.display()));
    }
}
