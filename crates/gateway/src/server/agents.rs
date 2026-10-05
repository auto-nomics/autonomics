//! Live agent control handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dag_core::dag::DagTuiSnapshot;
use runtime::control::AgentInfo;

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use crate::proto::*;

// ── agents ───────────────────────────────────────────────────────────

#[utoipa::path(get, path = "/api/v1/agents", tag = "agents", responses((status = 200, body = Object)))]
pub(crate) async fn list_agents(State(state): State<GatewayState>) -> Json<Vec<AgentInfo>> {
    Json(
        state
            .control
            .get_status()
            .await
            .map(|s| s.agents)
            .unwrap_or_default(),
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/agents",
    tag = "agents",
    request_body = SpawnAgentRequest,
    responses((status = 200, body = SpawnAgentResponse), (status = 409, description = "Agent name conflict or no model configured"))
)]
pub(crate) async fn spawn_agent(
    State(state): State<GatewayState>,
    Json(req): Json<SpawnAgentRequest>,
) -> GatewayResult<Json<SpawnAgentResponse>> {
    let parent_path: agentik_types::AgentPath = req
        .parent_path
        .parse()
        .map_err(|e| format!("invalid parent_path `{}`: {e}", req.parent_path))?;
    // Model override: unresolvable specs degrade to the daemon default,
    // matching the TUI's spawn behavior today.
    let model_override = match req.model_spec.as_deref() {
        Some(spec) => {
            let models = state.models.clone();
            Some(
                tokio::task::block_in_place(|| models.resolve(spec))
                    .map_err(GatewayError::Message)?,
            )
        }
        None => {
            // The host's model slot can be installed-but-empty (daemon
            // started with no active model configured). Rejecting here
            // surfaces a startup-class error at spawn instead of a turn
            // failure later.
            if state.model_slot.load_full().is_none() {
                return Err(GatewayError::Status(
                    StatusCode::CONFLICT,
                    "No model configured on host.".to_string(),
                ));
            }
            None
        }
    };
    let path = state
        .control
        .spawn_with_profile(&req.name, &parent_path, req.profile, model_override)
        .await
        .map_err(|e| GatewayError::Status(StatusCode::CONFLICT, e))?;
    Ok(Json(SpawnAgentResponse { path }))
}

/// Deliver a user message to an agent's next turn (the reply stream is the
/// SSE event channel, same contract the TUI drives today). The runtime
/// confirms enqueue before this returns: an unknown agent or an exited
/// agent loop is an error, not a silent 202.
#[utoipa::path(
    post,
    path = "/api/v1/agents/{name}/messages",
    tag = "agents",
    request_body = DeliverMessageRequest,
    responses(
        (status = 202, description = "Message accepted and enqueued on the agent's command channel"),
        (status = 404, description = "Agent is not in the live registry (re-spawn after restarts)"),
        (status = 503, description = "Agent loop has exited and cannot receive messages"),
        (status = 504, description = "Host runtime did not confirm delivery in time")
    )
)]
pub(crate) async fn deliver_message(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Json(req): Json<DeliverMessageRequest>,
) -> GatewayResult<StatusCode> {
    match state.control.deliver_message_tracked(&name, req.text).await {
        Some(Ok(())) => Ok(StatusCode::ACCEPTED),
        Some(Err(message)) if message.contains("is not registered") => {
            Err(GatewayError::Status(StatusCode::NOT_FOUND, message))
        }
        Some(Err(message)) => Err(GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, message)),
        None => Err(GatewayError::Status(
            StatusCode::GATEWAY_TIMEOUT,
            format!("host runtime did not confirm delivery to agent '{name}'"),
        )),
    }
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/cancel", tag = "agents", responses((status = 202, description = "Cancel requested")))]
pub(crate) async fn cancel_agent(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> StatusCode {
    state.control.cancel_agent(&name);
    StatusCode::ACCEPTED
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/compact", tag = "agents", responses((status = 202, description = "Compaction requested")))]
pub(crate) async fn compact_agent(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> StatusCode {
    state.control.compact_agent(&name);
    StatusCode::ACCEPTED
}

#[utoipa::path(post, path = "/api/v1/agents/{name}/shutdown", tag = "agents", responses((status = 202, description = "Shutdown requested")))]
pub(crate) async fn shutdown_agent(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> StatusCode {
    state.control.shutdown_agent(&name);
    StatusCode::ACCEPTED
}

#[utoipa::path(get, path = "/api/v1/agents/{name}/model", tag = "agents", responses((status = 200, body = AgentModelInfoView), (status = 404, description = "Agent not found")))]
pub(crate) async fn get_agent_model(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<AgentModelInfoView>> {
    let info = state.control.agent_model_info(&name).await.ok_or_else(|| {
        GatewayError::Status(StatusCode::NOT_FOUND, format!("agent `{name}` not found"))
    })?;
    let (model, context_length) = info;
    Ok(Json(AgentModelInfoView {
        model,
        context_length,
    }))
}

/// Hot-swap an agent's model and persist the preference in its stored
/// record (mirrors the TUI's `set_agent_model` + `persist_agent_model`).
#[utoipa::path(put, path = "/api/v1/agents/{name}/model", tag = "agents", request_body = SetAgentModelRequest, responses((status = 202, description = "Model updated"), (status = 400, description = "Model could not be resolved")))]
pub(crate) async fn set_agent_model(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Json(req): Json<SetAgentModelRequest>,
) -> GatewayResult<StatusCode> {
    let models = state.models.clone();
    let hub = state.hub.clone();
    let model =
        tokio::task::block_in_place(|| models.resolve_with_refresh_callback(&req.spec, &hub))
            .map_err(|e| GatewayError::Message(e.to_string()))?;
    state.control.set_agent_model(&name, model);

    // Persist preferred_model in the agent record (best-effort).
    let storage = state.infra.storage.clone();
    let name_for_task = name.clone();
    let spec = req.spec.clone();
    tokio::spawn(async move {
        if let Ok(Some(mut record)) = storage.get_agent_by_name(&name_for_task).await {
            if let Some(obj) = record.config_json.as_object_mut() {
                obj.insert(
                    "preferred_model".to_string(),
                    serde_json::Value::String(spec.clone()),
                );
            }
            record.last_active = chrono::Utc::now().timestamp_millis();
            if let Err(e) = storage.upsert_agent(record).await {
                tracing::warn!(agent = %name_for_task, error = %e, "failed to persist model spec");
            }
        }
    });
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(get, path = "/api/v1/agents/{name}/dag", tag = "agents", responses((status = 200, body = Object), (status = 500, description = "DAG snapshot failed")))]
pub(crate) async fn get_agent_dag(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<DagTuiSnapshot>> {
    // Pure SharedInfra operation — no host lock involved.
    let client = state.infra.engine_manager.client_for_session(&name);
    let snapshot = client
        .dag_tui_snapshot()
        .await
        .map_err(|e| GatewayError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(snapshot))
}

#[utoipa::path(get, path = "/api/v1/agents/{name}/config", tag = "agents", responses((status = 200, body = AgentRuntimeConfigView), (status = 404, description = "Agent not found")))]
pub(crate) async fn get_agent_config(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
) -> GatewayResult<Json<AgentRuntimeConfigView>> {
    let record = state
        .infra
        .storage
        .get_agent_by_name(&name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            GatewayError::Status(StatusCode::NOT_FOUND, format!("agent `{name}` not found"))
        })?;
    let profile: agentik_core::AgentProfile =
        serde_json::from_value(record.config_json).map_err(|e| e.to_string())?;
    let mut effective = state.infra.memory.runtime_config();
    if let Some(enabled) = profile.runtime.use_memory {
        effective.use_memory = enabled;
    }
    if let Some(enabled) = profile.runtime.generate_memory {
        effective.generate_memory = enabled;
    }
    Ok(Json(AgentRuntimeConfigView {
        runtime: profile.runtime,
        effective,
    }))
}

#[utoipa::path(put, path = "/api/v1/agents/{name}/config", tag = "agents", request_body = SetAgentRuntimeConfigRequest, responses((status = 200, body = AgentRuntimeConfigView), (status = 400, description = "Invalid agent configuration")))]
pub(crate) async fn set_agent_config(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Json(req): Json<SetAgentRuntimeConfigRequest>,
) -> GatewayResult<Json<AgentRuntimeConfigView>> {
    let mut record = state
        .infra
        .storage
        .get_agent_by_name(&name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            GatewayError::Status(StatusCode::NOT_FOUND, format!("agent `{name}` not found"))
        })?;
    let runtime = req.runtime;
    let mut profile: agentik_core::AgentProfile =
        serde_json::from_value(record.config_json).map_err(|e| e.to_string())?;
    profile.runtime = runtime.clone();
    record.config_json = serde_json::to_value(profile).map_err(|e| e.to_string())?;
    record.last_active = chrono::Utc::now().timestamp_millis();
    state
        .infra
        .storage
        .upsert_agent(record)
        .await
        .map_err(|e| e.to_string())?;

    let effective = state
        .control
        .set_agent_runtime_config(&name, runtime.clone())
        .await
        .map_err(|e| GatewayError::Message(e.to_string()))?;
    Ok(Json(AgentRuntimeConfigView { runtime, effective }))
}
