//! Live agent session handlers.

use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::state::GatewayState;

// ── context ─────────────────────────────────────────────────────────

/// Fire a `ListContexts` at the agent; the reply arrives as a
/// `ContextList` agent frame on the event stream (and folds into the
/// driver's session cache for future `GET /state` calls).
///
/// With one persistent context per agent the reply carries at most one
/// entry — the agent's sole context. Conversation create / switch / fork /
/// close within an agent are no longer HTTP operations; independent work
/// spawns another agent. Older histories stay readable through the storage
/// routes (`GET /storage/agents/{id}/sessions`, `GET .../history`).
#[utoipa::path(get, path = "/api/v1/agents/{name}/context", tag = "context", responses((status = 202, description = "Context-list request sent")))]
pub(crate) async fn request_context_list(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> StatusCode {
    state.control.list_contexts(&name);
    StatusCode::ACCEPTED
}
