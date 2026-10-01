//! Display setting handlers.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── settings ─────────────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/settings", tag = "settings", responses((status = 200, body = SettingsMap)))]
pub(crate) async fn get_settings(State(state): State<GatewayState>) -> Json<SettingsMap> {
    let display = tokio::task::block_in_place(|| state.models.display_settings());
    let settings = HashMap::from([
        (
            "collapse_thinking".to_string(),
            if display.collapse_thinking { "1" } else { "0" }.to_string(),
        ),
        (
            "collapse_tool_calls".to_string(),
            if display.collapse_tool_calls {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        (
            "collapse_tool_results".to_string(),
            if display.collapse_tool_results {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
    ]);
    Json(SettingsMap { settings })
}

#[utoipa::path(put, path = "/api/v1/settings", tag = "settings", request_body = PutSettingRequest, responses((status = 204, description = "Setting saved"), (status = 500, description = "Setting save failed")))]
pub(crate) async fn put_setting(
    State(state): State<GatewayState>,
    Json(req): Json<PutSettingRequest>,
) -> GatewayResult<StatusCode> {
    let models = state.models.clone();
    tokio::task::block_in_place(|| models.put_setting(&req.key, &req.value))
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
