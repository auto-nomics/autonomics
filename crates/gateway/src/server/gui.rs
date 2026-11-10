use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::get,
};

const GUI_INDEX: &str = include_str!("../../gui-dist/index.html");
const GUI_JAVASCRIPT: &str = include_str!("../../gui-dist/assets/gui.js");
const GUI_WORKER: &str = include_str!("../../gui-dist/assets/gui.mjs");
const GUI_STYLESHEET: &str = include_str!("../../gui-dist/assets/gui.css");

/// The browser frontend.
///
/// The page is a static shell: no credential is embedded in it. The
/// SPA authenticates with the `HttpOnly` session cookie it gets from
/// `POST /api/session`, so the daemon's bearer token never appears in
/// the HTML, in a JS global, or in the browser's devtools.
pub fn router() -> Router {
    Router::new()
        .route("/", get(gui_index))
        // Headless-export print page: same SPA shell, the bundle
        // dispatches on the pathname (features/deck print entry).
        .route("/deck/print", get(gui_index))
        .route("/deck/presenter", get(gui_index))
        .route("/assets/gui.js", get(gui_javascript))
        .route("/assets/gui.mjs", get(gui_worker))
        .route("/assets/gui.css", get(gui_stylesheet))
        .route(
            "/excalidraw/fonts/Liberation/{file}",
            get(gui_scientific_font),
        )
}

async fn gui_index() -> Html<&'static str> {
    Html(GUI_INDEX)
}

async fn gui_javascript() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        GUI_JAVASCRIPT,
    )
}

async fn gui_worker() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        GUI_WORKER,
    )
}

async fn gui_stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        GUI_STYLESHEET,
    )
}

// Excalidraw resolves its pinned font through EXCALIDRAW_ASSET_PATH. Keep
// this allowlist embedded alongside the GUI; no runtime filesystem or CDN.
async fn gui_scientific_font(Path(file): Path<String>) -> axum::response::Response {
    let (mime, bytes): (&str, &'static [u8]) = match file.as_str() {
        "LiberationSans-Regular.woff2" => (
            "font/woff2",
            include_bytes!(
                "../../gui-dist/excalidraw/fonts/Liberation/LiberationSans-Regular.woff2"
            ),
        ),
        "LiberationSans-Regular.sources.json" => (
            "application/json",
            include_bytes!(
                "../../gui-dist/excalidraw/fonts/Liberation/LiberationSans-Regular.sources.json"
            ),
        ),
        "OFL.txt" => (
            "text/plain; charset=utf-8",
            include_bytes!("../../gui-dist/excalidraw/fonts/Liberation/OFL.txt"),
        ),
        "NotoSansSC-OFL.txt" => (
            "text/plain; charset=utf-8",
            include_bytes!("../../gui-dist/excalidraw/fonts/Liberation/NotoSansSC-OFL.txt"),
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn index_carries_no_credential() {
        let response = router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        // No bootstrap script, no token placeholder: the page is a
        // static shell and the SPA authenticates with a cookie.
        assert!(!body.contains("__AUTONOMICS_GATEWAY_TOKEN__"), "{body}");
        assert!(!body.to_lowercase().contains("token"), "{body}");
    }

    #[tokio::test]
    async fn print_shell_carries_no_credential() {
        // /deck/print serves the same static shell for the headless
        // export page — no token may leak into it.
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/deck/print")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(!body.contains("__AUTONOMICS_GATEWAY_TOKEN__"), "{body}");
        assert!(!body.to_lowercase().contains("token"), "{body}");
    }

    #[tokio::test]
    async fn presenter_shell_is_available() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/deck/presenter")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
    }

    #[tokio::test]
    async fn serves_embedded_assets() {
        let app = router();
        for uri in ["/assets/gui.js", "/assets/gui.mjs", "/assets/gui.css"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(response.status().is_success(), "{uri}");
        }
    }
    #[tokio::test]
    async fn serves_pinned_scientific_font_and_notices_without_filesystem_fallback() {
        for (file, mime) in [
            ("LiberationSans-Regular.woff2", "font/woff2"),
            ("LiberationSans-Regular.sources.json", "application/json"),
            ("OFL.txt", "text/plain; charset=utf-8"),
            ("NotoSansSC-OFL.txt", "text/plain; charset=utf-8"),
        ] {
            let response = router()
                .oneshot(
                    Request::builder()
                        .uri(format!("/excalidraw/fonts/Liberation/{file}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            if file.ends_with("woff2") {
                assert!(body.starts_with(b"wOF2"));
                assert_eq!(
                    &body[..],
                    include_bytes!(
                        "../../gui-dist/excalidraw/fonts/Liberation/LiberationSans-Regular.woff2"
                    )
                );
            }
        }
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/excalidraw/fonts/Liberation/private.ttf")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
