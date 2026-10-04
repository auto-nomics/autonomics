//! Live agent session handlers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use super::state::GatewayState;
use crate::proto::*;

// ── sessions ─────────────────────────────────────────────────────────

/// Fire a `ListSessions` at the agent; the reply arrives as a
/// `SessionList` agent frame on the event stream (and folds into the
/// driver's session cache for future `GET /state` calls).
#[utoipa::path(get, path = "/api/v1/agents/{name}/sessions", tag = "sessions", responses((status = 202, description = "Session-list request sent")))]
pub(crate) async fn request_session_list(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> StatusCode {
    state.control.list_sessions(&name);
    StatusCode::ACCEPTED
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/sessions", tag = "sessions", request_body = CreateSessionRequest, responses((status = 202, description = "Session creation accepted")))]
pub(crate) async fn create_session(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Json(req): Json<CreateSessionRequest>,
) -> StatusCode {
    state
        .control
        .create_session(&name, req.title, req.fork_from);
    StatusCode::ACCEPTED
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/sessions/{id}/activate", tag = "sessions", responses((status = 202, description = "Session activation requested")))]
pub(crate) async fn activate_session(
    State(state): State<GatewayState>,
    Path((name, id)): Path<(String, Uuid)>,
) -> StatusCode {
    state.control.switch_session(&name, id);
    StatusCode::ACCEPTED
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/sessions/{id}/close", tag = "sessions", responses((status = 202, description = "Session close requested")))]
pub(crate) async fn close_session(
    State(state): State<GatewayState>,
    Path((name, id)): Path<(String, Uuid)>,
) -> StatusCode {
    state.control.close_session(&name, id);
    StatusCode::ACCEPTED
}

#[utoipa::path(patch, path = "/api/v1/agents/{name}/sessions/{id}/title", tag = "sessions", request_body = RenameSessionRequest, responses((status = 202, description = "Session rename requested")))]
pub(crate) async fn rename_session(
    State(state): State<GatewayState>,
    Path((name, id)): Path<(String, Uuid)>,
    Json(req): Json<RenameSessionRequest>,
) -> StatusCode {
    state.control.rename_session(&name, id, req.title);
    StatusCode::ACCEPTED
}
