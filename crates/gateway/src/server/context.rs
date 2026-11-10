//! Live agent session handlers.

use agentik_types::ContextInfo;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::AgentContextResponse;

// ── context ─────────────────────────────────────────────────────────

/// Return the agent's identity and sole persisted context without activating
/// a dormant instance. Also fire a `ListContexts` at a live agent; its reply
/// arrives as a
/// `ContextList` agent frame on the event stream (and folds into the
/// driver's session cache for future `GET /state` calls).
///
/// With one persistent context per agent the reply carries at most one
/// entry — the agent's sole context. Conversation create / switch / fork /
/// close within an agent are no longer HTTP operations; independent work
/// spawns another agent. Older histories stay readable through the storage
/// routes (`GET /storage/agents/{id}/sessions`, `GET .../history`).
#[utoipa::path(get, path = "/api/v1/agents/{name}/context", tag = "context", responses((status = 202, body = Object, description = "Identity and sole context; live context-list refresh requested"), (status = 404, description = "Agent not found"), (status = 500, description = "Context storage query failed")))]
pub(crate) async fn request_context_list(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<(StatusCode, Json<AgentContextResponse>)> {
    let live_id = state
        .control
        .get_status()
        .await
        .and_then(|status| status.agents.into_iter().find(|agent| agent.path == name))
        .and_then(|agent| agent.agent_id);
    let storage = state.infra.storage.as_ref();
    // A just-registered instance already knows its identity even if its
    // registry write is still in flight. Dormant views use an exact lookup.
    let agent_id = match live_id {
        Some(id) => id,
        None => {
            storage
                .get_agent_by_name(&name)
                .await
                .map_err(|error| {
                    GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                })?
                .ok_or_else(|| {
                    GatewayError::Status(StatusCode::NOT_FOUND, format!("agent {name} not found"))
                })?
                .id
        }
    };
    let cached = live_id.and_then(|_| state.contexts.snapshot().remove(&name));
    let context = if cached.is_some() {
        cached
    } else {
        let records = storage
            .list_session_records(agent_id)
            .await
            .map_err(|error| {
                GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?;
        let bound_id = storage.agent_context_id(agent_id).await.map_err(|error| {
            GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        })?;
        agentik_core::storage::select_agent_context_record(bound_id, records)
            .map_err(|error| {
                GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?
            .map(|record| ContextInfo {
                id: record.session_id,
                title: record.title,
                created_at: record.started_at,
                last_active: record.last_active,
                telemetry: record.telemetry,
                parent_session_id: record.parent_session_id,
                fork_point_message_id: record.fork_point_message_id,
            })
    };
    if live_id.is_some() {
        state.control.list_contexts(&name);
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(AgentContextResponse { agent_id, context }),
    ))
}
