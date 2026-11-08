//! Operator visibility and adoption of existing job intents. No submission API.
use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) async fn list(State(state): State<GatewayState>) -> GatewayResult<Json<Value>> {
    let records = state
        .infra
        .hpc_monitor
        .as_ref()
        .map(|hub| hub.views(None))
        .transpose()
        .map_err(|_| {
            GatewayError::Status(
                StatusCode::INTERNAL_SERVER_ERROR,
                "HPC monitor inventory unavailable".into(),
            )
        })?
        .unwrap_or_default();
    Ok(Json(
        json!({"monitors":records,"scientific_admission_granted":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WatchExisting {
    target_id: Option<String>,
    target_revision: Option<u64>,
    agent_path: String,
    agent_id: uuid::Uuid,
    session_id: uuid::Uuid,
    submission_key: String,
    cluster: Option<String>,
}
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
    let hub = state.infra.hpc_monitor.as_ref().ok_or_else(|| {
        GatewayError::Status(StatusCode::CONFLICT, "HPC monitoring is disabled".into())
    })?;
    let key = match (&input.target_id, input.target_revision) {
        (Some(id), Some(revision)) => format!("{id}-r{revision}"),
        (None, None) => "legacy-slurm-r1".into(),
        _ => {
            return Err(GatewayError::Message(
                "target_id and target_revision must be supplied together".into(),
            ));
        }
    };
    let target_hub = hub.target_hub(&key).map_err(GatewayError::Message)?;
    let record=target_hub.watch_existing(input.cluster.as_deref(),&input.submission_key,runtime::hpc_monitor::Owner{agent_id:input.agent_id.to_string(),agent_path:input.agent_path,session_id:input.session_id.to_string()},json!({"operator_attachment":"Verify the existing expected outputs under the original frozen contract; no new computation."})).await.map_err(|_|GatewayError::Status(StatusCode::CONFLICT,"Cannot bind the existing durable submission; no job was submitted".into()))?;
    let mut view = record.view();
    if let Some((id, revision)) = key.rsplit_once("-r") {
        view.target_id = Some(id.into());
        view.target_revision = revision.parse().ok();
    }
    Ok(Json(
        json!({"monitor":view,"new_job_submitted":false,"scientific_admission_granted":false}),
    ))
}

/// Operator control of observation only. The authenticated gateway owner must
/// bind the exact target revision and original conversation, never a new job.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopMonitor {
    target_id: String,
    target_revision: u64,
    agent_id: uuid::Uuid,
    agent_path: String,
    session_id: uuid::Uuid,
    submission_key: String,
    reason: String,
}

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
    let hub = state.infra.hpc_monitor.as_ref().ok_or_else(|| {
        GatewayError::Status(StatusCode::CONFLICT, "HPC monitoring is disabled".into())
    })?;
    let prior = hub
        .views(None)
        .map_err(GatewayError::Message)?
        .into_iter()
        .find(|view| {
            view.id == id
                && view.target_id.as_deref() == Some(input.target_id.as_str())
                && view.target_revision == Some(input.target_revision)
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
        .target_hub(&format!("{}-r{}", input.target_id, input.target_revision))
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
    view.target_revision = Some(input.target_revision);
    Ok(Json(
        json!({"monitor": view, "prior_monitor": prior, "operator_review": input.reason,
        "remote_job_cancelled": false, "new_job_submitted": false,
        "original_submission_outcome_resolved": false, "scientific_admission_granted": false}),
    ))
}
