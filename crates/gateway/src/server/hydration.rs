//! Frontend bootstrap handlers.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── hydration ────────────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/state", tag = "hydration", responses((status = 200, body = StateSnapshot)))]
pub(crate) async fn get_state(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<StateSnapshot>> {
    let models = state.models.clone();
    let catalog = tokio::task::block_in_place(|| models.catalog())
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let live_agents = state
        .control
        .get_status()
        .await
        .map(|s| s.agents)
        .unwrap_or_default();
    let display_settings = tokio::task::block_in_place(|| state.models.display_settings());
    // Serialize the snapshot body first, then read `last_seq` — the
    // hydration protocol (connect → snapshot → apply live frames with
    // seq > last_seq) tolerates extra frames but never gaps.
    let snapshot = StateSnapshot {
        last_seq: 0, // filled below
        active_model_spec: catalog.active_model.clone(),
        profiles: agentik_core::AgentKind::ALL.to_vec(),
        agents: live_agents,
        sessions: state.sessions.snapshot(),
        display_settings,
        runtime_defaults: state.infra.memory.runtime_config(),
        model_catalog: catalog,
    };
    let last_seq = state.hub.last_seq();
    Ok(Json(StateSnapshot {
        last_seq,
        ..snapshot
    }))
}

#[utoipa::path(get, path = "/api/v1/profiles", tag = "hydration", responses((status = 200, body = Object)))]
pub(crate) async fn get_profiles() -> Json<Vec<agentik_core::AgentKind>> {
    Json(agentik_core::AgentKind::ALL.to_vec())
}
