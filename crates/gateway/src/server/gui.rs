use axum::{
    Router,
    http::header,
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
        .route("/assets/gui.js", get(gui_javascript))
        .route("/assets/gui.mjs", get(gui_worker))
        .route("/assets/gui.css", get(gui_stylesheet))
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
