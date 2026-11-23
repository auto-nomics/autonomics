//! Operator visibility and adoption of existing job intents. No submission API.
use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/hpc/jobs", get(list))
        .route("/hpc/jobs/watch", post(watch))
        .route("/hpc/jobs/{id}/stop-monitor", post(stop_monitor))
}

#[utoipa::path(
    get, path = "/api/v1/hpc/jobs", tag = "hpc", operation_id = "list_hpc_jobs",
    responses((status = 200, body = Object), (status = 500, description = "HPC monitor inventory unavailable"))
)]
pub(crate) async fn list(State(state): State<GatewayState>) -> GatewayResult<Json<Value>> {
    let records = state.infra.compute.monitors.views(None).map_err(|_| {
        GatewayError::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "HPC monitor inventory unavailable".into(),
        )
    })?;
    Ok(Json(
        json!({"monitors":records,"scientific_admission_granted":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WatchExisting {
    target_id: Option<String>,
    agent_path: String,
    agent_id: uuid::Uuid,
    session_id: uuid::Uuid,
    submission_key: String,
    cluster: Option<String>,
}
#[utoipa::path(
    post, path = "/api/v1/hpc/jobs/watch", tag = "hpc", request_body = Object,
    responses((status = 200, body = Object), (status = 409, description = "HPC operation unavailable or conflicts with current state"))
)]
pub(crate) async fn watch(
    State(state): State<GatewayState>,
    Json(input): Json<WatchExisting>,
) -> GatewayResult<Json<Value>> {
    let deny = || {
        GatewayError::Status(
            StatusCode::CONFLICT,
            "Existing job must be bound to its original registered agent and current context"
                .into(),
        )
    };
    let agent = state
        .infra
        .storage
        .get_agent(input.agent_id)
        .await
        .map_err(|_| deny())?
        .ok_or_else(deny)?;
    if agent.name != input.agent_path
        || state
            .infra
            .storage
            .agent_context_id(input.agent_id)
            .await
            .map_err(|_| deny())?
            != Some(input.session_id)
    {
        return Err(deny());
    }
    let key = input
        .target_id
        .clone()
        .unwrap_or_else(|| "legacy-slurm".into());
    let owner = runtime::hpc_monitor::Owner {
        agent_id: input.agent_id.to_string(),
        agent_path: input.agent_path,
        session_id: input.session_id.to_string(),
    };
    let plan = json!({"operator_attachment":"Verify the existing expected outputs under the original frozen contract; no new computation."});
    let result = state
        .infra
        .compute
        .watch_existing_job(
            &key,
            input.cluster.as_deref(),
            &input.submission_key,
            owner,
            plan,
        )
        .await;
    let record = result.map_err(|_| {
        GatewayError::Status(
            StatusCode::CONFLICT,
            "Cannot bind the existing durable submission; no job was submitted".into(),
        )
    })?;
    let mut view = record.view();
    view.target_id = Some(key);
    Ok(Json(
        json!({"monitor":view,"new_job_submitted":false,"scientific_admission_granted":false}),
    ))
}

/// Operator control of observation only. The authenticated gateway owner must
/// bind the original target and original conversation, never a new job.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopMonitor {
    target_id: String,
    agent_id: uuid::Uuid,
    agent_path: String,
    session_id: uuid::Uuid,
    submission_key: String,
    reason: String,
}

#[utoipa::path(
    post, path = "/api/v1/hpc/jobs/{id}/stop-monitor", tag = "hpc", request_body = Object,
    params(("id" = String, Path)),
    responses((status = 200, body = Object), (status = 409, description = "HPC operation unavailable or conflicts with current state"))
)]
pub(crate) async fn stop_monitor(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(input): Json<StopMonitor>,
) -> GatewayResult<Json<Value>> {
    if input.reason.trim().is_empty() || input.reason.len() > 2048 {
        return Err(GatewayError::Message(
            "bounded operator review reason required".into(),
        ));
    }
    let hub = &state.infra.compute.monitors;
    let prior = hub
        .views(None)
        .map_err(GatewayError::Message)?
        .into_iter()
        .find(|view| {
            view.id == id
                && view.target_id.as_deref() == Some(input.target_id.as_str())
                && view.agent_id == input.agent_id.to_string()
                && view.agent_path == input.agent_path
                && view.session_id == input.session_id.to_string()
                && view.submission_key == input.submission_key
        })
        .ok_or_else(|| {
            GatewayError::Status(
                StatusCode::CONFLICT,
                "monitor must match its exact target and original conversation".into(),
            )
        })?;
    let target = hub
        .target_hub(&input.target_id)
        .map_err(GatewayError::Message)?;
    let owner = runtime::hpc_monitor::Owner {
        agent_id: input.agent_id.to_string(),
        agent_path: input.agent_path,
        session_id: input.session_id.to_string(),
    };
    let mut view = target
        .stop(&id, &owner)
        .map_err(GatewayError::Message)?
        .view();
    view.target_id = Some(input.target_id);
    Ok(Json(
        json!({"monitor": view, "prior_monitor": prior, "operator_review": input.reason,
        "remote_job_cancelled": false, "new_job_submitted": false,
        "original_submission_outcome_resolved": false, "scientific_admission_granted": false}),
    ))
}
