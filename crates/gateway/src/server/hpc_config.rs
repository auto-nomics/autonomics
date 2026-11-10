//! Compatibility endpoint for editing the selected Slurm compute target.
use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use axum::{Json, extract::State, http::StatusCode};
use runtime::compute::{ComputeBackend, SaveTarget};
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
        .find(|target| target.id == snapshot.policy.target_id)
        .ok_or_else(|| conflict("selected target missing"))?;
    let settings = match &target.backend {
        ComputeBackend::Slurm { settings } => settings,
        _ => {
            return Ok(Json(json!({"target_id":target.id,"editable":false})));
        }
    };
    Ok(Json(
        json!({"target_id":target.id,"editable":true,"config":settings}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Update {
    target_id: String,
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
    if snapshot.policy.target_id != request.target_id {
        return Err(conflict("compute selection changed; refresh before saving"));
    }
    let target = snapshot
        .targets
        .iter()
        .find(|target| target.id == request.target_id)
        .ok_or_else(|| conflict("selected target missing"))?;
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
            id: target.id.clone(),
            name: target.name.clone(),
            enabled: target.enabled,
            backend: ComputeBackend::Slurm { settings },
            data_domains: target.data_domains.clone(),
        })
        .map_err(conflict)?;
    super::compute::changed(&state, &saved);
    Ok(Json(json!({"target_id":target.id})))
}
