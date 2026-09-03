//! Static hosting for the autonomics-web SPA.
//!
//! The built frontend (`apps/web/dist`, produced by `scripts/build-web.sh`)
//! is served by the same binary that owns the API. rust-embed embeds the
//! assets in release builds; in debug builds it reads them from disk so a
//! frontend rebuild does not require recompiling Rust.

use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
// Relative folders resolve against this crate's manifest directory. (A
// $CARGO_MANIFEST_DIR literal would need the interpolate-folder-path feature.)
#[folder = "../../apps/web/dist"]
struct WebAssets;

/// Directories holding build output whose filenames are content-hashed (or,
/// like the PDFium WASM binary, byte-stable across releases).
const ASSET_DIRS: [&str; 3] = ["assets/", "wasm/", "icons/"];

/// Mount the SPA. Routes are kept outside the bearer-auth layer: static
/// assets carry no secrets and the SPA must be able to load before it can
/// prompt for a token.
pub(crate) fn router() -> Router {
    Router::new()
        .route("/", get(index))
        // `any`, not `get`: a POST to an unmounted /api path is a 404, not a
        // method-router 405.
        .fallback(any(file_or_shell))
}

async fn index() -> Response {
    serve_asset("index.html").expect("index.html always exists in a built dist")
}

/// Every unrouted GET lands here: serve a real file, keep API misses as JSON
/// 404s, and fall back to the SPA shell for client-side routes.
async fn file_or_shell(request: Request) -> Response {
    let path = request.uri().path();

    if path.starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "not found" })),
        )
            .into_response();
    }

    let relative = path.trim_start_matches('/');
    // Stale HTML referencing a no-longer-existing chunk needs a cache-miss
    // signal, not an HTML body answering a JS request.
    let asset_namespace = ASSET_DIRS.iter().any(|dir| relative.starts_with(dir));

    if let Some(response) = serve_asset(relative) {
        return response;
    }
    if asset_namespace {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    // Client-side routing entry: let the SPA render the route itself.
    serve_asset("index.html").expect("index.html always exists in a built dist")
}

fn serve_asset(path: &str) -> Option<Response> {
    // Reject traversal before the map lookup; embed keys are plain relative
    // paths and never contain `..`.
    if path.is_empty() || path.split('/').any(|segment| segment == "..") {
        return None;
    }

    let embedded = WebAssets::get(path)?;

    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let mut headers = HeaderMap::new();
    // Vite content-hashes everything under the asset directories; index.html
    // and anything at the dist root must always revalidate.
    let cache_control = if ASSET_DIRS.iter().any(|dir| path.starts_with(dir)) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    if let Ok(value) = header::HeaderValue::from_str(cache_control) {
        headers.insert(header::CACHE_CONTROL, value);
    }

    Some(
        (
            headers,
            [(header::CONTENT_TYPE, content_type)],
            embedded.data.into_owned(),
        )
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(uri: &str) -> Response {
        router()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn serves_index_html_with_no_cache() {
        let response = get("/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/html"));
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
    }

    #[tokio::test]
    async fn serves_wasm_with_application_wasm_type() {
        let response = get("/wasm/pdfium.wasm").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/wasm"
        );
    }

    #[tokio::test]
    async fn hashed_assets_get_immutable_cache_headers() {
        // Pick any real file under assets/ from the embedded set.
        let some_asset = WebAssets::iter()
            .find(|path| path.starts_with("assets/"))
            .expect("built dist contains hashed assets");
        let response = get(&format!("/{some_asset}")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn spa_fallback_returns_the_shell_for_unknown_routes() {
        let response = get("/paper/whatever").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("<div id=\"root\">"));
    }

    #[tokio::test]
    async fn missing_hashed_asset_is_a_clean_404() {
        let response = get("/assets/js/does-not-exist-abc123.js").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unmatched_api_paths_stay_json_404s() {
        let response = get("/api/v1/bib/nope").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body["error"].is_string());
    }
}
