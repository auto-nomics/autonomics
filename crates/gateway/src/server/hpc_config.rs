//! Compatibility endpoint for editing the selected Slurm compute target.
use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use axum::{Json, extract::State, http::StatusCode};
use runtime::compute::{ComputeBackend, SaveTarget, SelectTarget};
use serde::Deserialize;
use serde_json::{Value, json};

fn conflict(message: impl Into<String>) -> GatewayError {
    GatewayError::Status(StatusCode::CONFLICT, message.into())
}

pub(crate) async fn show(State(state): State<GatewayState>) -> GatewayResult<Json<Value>> {
    let registry = state
        .infra
        .compute
        .as_ref()
        .ok_or_else(|| conflict("compute registry unavailable"))?;
    let snapshot = registry.state();
    let target = snapshot
        .targets
        .iter()
        .find(|target| {
            target.id == snapshot.policy.target_id
                && target.revision == snapshot.policy.target_revision
        })
        .ok_or_else(|| conflict("selected target revision missing"))?;
    let settings = match &target.backend {
        ComputeBackend::Slurm { settings } => settings,
        _ => {
            return Ok(Json(
                json!({"target_id":target.id,"target_revision":target.revision,"registry_revision":snapshot.revision,"editable":false}),
            ));
        }
    };
    Ok(Json(
        json!({"target_id":target.id,"target_revision":target.revision,"registry_revision":snapshot.revision,"editable":true,"config":settings}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Update {
    target_id: String,
    target_revision: u64,
    expected_revision: u64,
    config: Value,
}

pub(crate) async fn update(
    State(state): State<GatewayState>,
    Json(request): Json<Update>,
) -> GatewayResult<Json<Value>> {
    let registry = state
        .infra
        .compute
        .as_ref()
        .ok_or_else(|| conflict("compute registry unavailable"))?;
    let snapshot = registry.state();
    if snapshot.revision != request.expected_revision
        || snapshot.policy.target_id != request.target_id
        || snapshot.policy.target_revision != request.target_revision
    {
        return Err(conflict("compute selection changed; refresh before saving"));
    }
    let target = snapshot
        .targets
        .iter()
        .find(|target| target.id == request.target_id && target.revision == request.target_revision)
        .ok_or_else(|| conflict("selected target revision missing"))?;
    let settings = match &target.backend {
        ComputeBackend::Slurm { settings } => settings,
        _ => return Err(conflict("selected target is not Slurm")),
    };
    let mut merged = serde_json::to_value(settings).map_err(|e| conflict(e.to_string()))?;
    let changes = request
        .config
        .as_object()
        .ok_or_else(|| conflict("config must be an object"))?;
    let validation: runtime::SlurmSiteProfile = serde_json::from_value(request.config.clone())
        .map_err(|e| conflict(format!("invalid settings: {e}")))?;
    validation.validate().map_err(conflict)?;
    let object = merged.as_object_mut().unwrap();
    for (key, value) in changes {
        if !object.contains_key(key) {
            return Err(conflict(format!("unknown setting: {key}")));
        }
        object.insert(key.clone(), value.clone());
    }
    let settings =
        serde_json::from_value(merged).map_err(|e| conflict(format!("invalid settings: {e}")))?;
    let saved = registry
        .save_target(SaveTarget {
            expected_revision: snapshot.revision,
            id: target.id.clone(),
            name: target.name.clone(),
            enabled: target.enabled,
            backend: ComputeBackend::Slurm { settings },
            data_domains: target.data_domains.clone(),
        })
        .map_err(conflict)?;
    let selected = registry
        .select(SelectTarget {
            expected_revision: saved.revision,
            target_id: target.id.clone(),
        })
        .map_err(|e| {
            conflict(format!(
                "target saved, but selection failed: {e}; select the new revision manually"
            ))
        })?;
    super::compute::changed(&state, &selected);
    Ok(Json(
        json!({"target_id":target.id,"target_revision":selected.policy.target_revision,"registry_revision":selected.revision}),
    ))
}
