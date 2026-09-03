//! Static hosting for the autonomics-web SPA.
//!
//! The built frontend (`apps/web/dist`, produced by `scripts/build-web.sh`)
//! is served by the same binary that owns the API. rust-embed embeds the
//! assets in release builds; in debug builds it reads them from disk so a
//! frontend rebuild does not require recompiling Rust.

use axum::Router;
use axum::extract::{Path, Request};
use axum::http::{StatusCode, header};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../apps/web/dist"]
struct WebAssets;

/// Mount the SPA. Routes are kept outside the bearer-auth layer: static
/// assets carry no secrets and the SPA must be able to load before it can
/// prompt for a token.
pub(crate) fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .route("/wasm/{*path}", get(asset))
        .route("/icons/{*path}", get(asset))
        .fallback(get(spa_fallback))
}

async fn index() -> Response {
    serve_asset("index.html")
}

/// Hashed build artifacts under `/assets` (and the other top-level asset
/// directories) — safe to cache aggressively.
async fn asset(Path(path): Path<String>) -> Response {
    serve_asset(&path)
}

/// Client-side routing entry: any non-API GET that did not match a real file
/// re-serves the shell so the SPA can render the route itself. Unmatched API
/// paths must stay JSON 404s — this fallback also sees them once merged.
async fn spa_fallback(request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "not found" })),
        )
            .into_response();
    }
    serve_asset("index.html")
}

fn serve_asset(path: &str) -> Response {
    // Reject traversal before rust-embed's map lookup; its keys are plain
    // relative paths and never contain `..`, so such lookups would miss anyway.
    if path.split('/').any(|segment| segment == "..") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let Some(embedded) = WebAssets::get(path) else {
        // Unknown hashed asset names should not fall back to the SPA shell:
        // a stale HTML page referencing a no-longer-existing chunk needs a
        // cache-miss signal, not a HTML body for a JS request.
        if path == "index.html" {
            return (StatusCode::NOT_FOUND, "frontend not built").into_response();
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let mut headers = HeaderMap::new();
    // Vite content-hashes everything under assets/ and wasm/; index.html and
    // anything at the dist root must always revalidate.
    let cache_control = if path.starts_with("assets/") || path.starts_with("wasm/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    if let Ok(value) = header::HeaderValue::from_str(cache_control) {
        headers.insert(header::CACHE_CONTROL, value);
    }

    (headers, [(header::CONTENT_TYPE, content_type)], embedded.data.into_owned()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_index_html_with_no_cache() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
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
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/wasm/pdfium.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/wasm"
        );
    }

    #[tokio::test]
    async fn spa_fallback_returns_the_shell_for_unknown_routes() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/paper/whatever")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("<div id=\"root\">"));
    }

    #[tokio::test]
    async fn missing_hashed_asset_is_a_clean_404() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/assets/js/does-not-exist-abc123.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
