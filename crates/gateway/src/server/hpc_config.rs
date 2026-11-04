//! Operator view and edit of the HPC (Slurm) site configuration.
//!
//! The daemon's ssh targets and executors are wired once at boot from
//! `RuntimeConfig::slurm` (site profile > env > default, plus an enrolled
//! compute catalog). This endpoint therefore serves the *effective*
//! snapshot and accepts edits by writing the operator site profile —
//! `PUT` never mutates the running executor; the response carries
//! `pending_restart` so panels can surface the reboot requirement.

use super::error::{GatewayError, GatewayResult};
use super::state::GatewayState;
use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};

/// Defensive cap on manifest passthrough — the operator manifest is a
/// small digest→SIF map, and the endpoint must not become a file reader.
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

pub(crate) async fn show(State(state): State<GatewayState>) -> GatewayResult<Json<Value>> {
    let infra = &state.infra;
    let config = &infra.slurm_config;

    let apptainer_manifest = config.apptainer_manifest.as_ref().map(|path| {
        let metadata = std::fs::metadata(path);
        let too_big = metadata
            .as_ref()
            .is_ok_and(|m| m.len() > MAX_MANIFEST_BYTES);
        let images = if too_big {
            None
        } else {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        };
        json!({
            "path": path.display().to_string(),
            "exists": path.exists(),
            "readable": images.is_some(),
            "images": images,
        })
    });

    let profile_path = runtime::SlurmSiteProfile::path(&infra.state_dir);
    let profile_on_disk = runtime::SlurmSiteProfile::load(&infra.state_dir).map_err(|error| {
        GatewayError::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("site profile unusable: {error}"),
        )
    })?;
    let pending_restart = profile_on_disk != infra.slurm_site_profile_boot;

    Ok(Json(json!({
        "config": config,
        "apptainer_manifest": apptainer_manifest,
        "site_profile": {
            "path": profile_path.display().to_string(),
            "exists": profile_path.exists(),
        },
        "pending_restart": pending_restart,
    })))
}

pub(crate) async fn update(
    State(state): State<GatewayState>,
    Json(profile): Json<runtime::SlurmSiteProfile>,
) -> GatewayResult<Json<Value>> {
    profile.validate().map_err(|error| {
        GatewayError::Status(
            StatusCode::BAD_REQUEST,
            format!("invalid site profile: {error}"),
        )
    })?;
    let path = profile.store(&state.infra.state_dir).map_err(|error| {
        GatewayError::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("persist site profile: {error}"),
        )
    })?;
    let pending_restart = Some(&profile) != state.infra.slurm_site_profile_boot.as_ref();
    tracing::info!(
        path = %path.display(),
        pending_restart,
        "HPC site profile updated"
    );
    Ok(Json(json!({
        "path": path.display().to_string(),
        "pending_restart": pending_restart,
    })))
}
