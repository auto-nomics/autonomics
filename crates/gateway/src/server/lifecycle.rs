//! Daemon lifecycle and API metadata handlers.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use super::state::GatewayState;
use crate::proto::*;

// ── daemon lifecycle ─────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/gateway/status", tag = "gateway", responses((status = 200, body = GatewayStatus)))]
pub(crate) async fn gateway_status(State(state): State<GatewayState>) -> Json<GatewayStatus> {
    Json(GatewayStatus {
        pid: std::process::id(),
        addr: state
            .addr
            .load_full()
            .map(|addr| addr.to_string())
            .unwrap_or_default(),
        version: state.version.to_string(),
        uptime_secs: state.started.elapsed().as_secs(),
        last_seq: state.hub.last_seq(),
        agent_count: state.sessions.snapshot().len(),
    })
}

#[utoipa::path(post, path = "/api/v1/gateway/shutdown", tag = "gateway", responses((status = 202, description = "Shutdown requested")))]
pub(crate) async fn gateway_shutdown(State(state): State<GatewayState>) -> StatusCode {
    state.shutdown.cancel();
    StatusCode::ACCEPTED
}

pub(crate) async fn api_index() -> Json<serde_json::Value> {
    Json(json!({
        "name": "autonomics-gateway",
        "version": 1,
        "modules": ["gateway", "agents", "sessions", "storage", "model-config", "bib"],
        "swagger_url": "/swagger-ui",
        "openapi_url": "/api/v1/api-docs/openapi.json",
    }))
}
