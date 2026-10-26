use axum::{
    Router,
    extract::State,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
};

const GUI_INDEX: &str = include_str!("../../gui-dist/index.html");
const GUI_JAVASCRIPT: &str = include_str!("../../gui-dist/assets/gui.js");
const GUI_WORKER: &str = include_str!("../../gui-dist/assets/gui.mjs");
const GUI_STYLESHEET: &str = include_str!("../../gui-dist/assets/gui.css");

pub fn router(gateway_token: String) -> Router {
    Router::new()
        .route("/", get(gui_index))
        .route("/assets/gui.js", get(gui_javascript))
        .route("/assets/gui.mjs", get(gui_worker))
        .route("/assets/gui.css", get(gui_stylesheet))
        .with_state(gateway_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn index_injects_loopback_gateway_token() {
        let response = router("secret-token".to_owned())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("window.__AUTONOMICS_GATEWAY_TOKEN__"));
        assert!(body.contains("\"secret-token\""));
    }

    #[tokio::test]
    async fn serves_embedded_assets() {
        let app = router("secret-token".to_owned());
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

async fn gui_index(State(gateway_token): State<String>) -> Html<String> {
    let token = serde_json::to_string(&gateway_token).unwrap_or_else(|_| "\"\"".to_owned());
    let token = token.replace("</", "<\\/");
    let bootstrap = format!("<script>window.__AUTONOMICS_GATEWAY_TOKEN__ = {token};</script>");
    Html(GUI_INDEX.replace("<head>", &format!("<head>{bootstrap}")))
}

async fn gui_javascript() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        GUI_JAVASCRIPT,
    )
}

async fn gui_worker() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        GUI_WORKER,
    )
}

async fn gui_stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        GUI_STYLESHEET,
    )
}
