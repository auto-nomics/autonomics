//! Operator compute/data configuration, independent from conversation state.
use super::{
    error::{GatewayError, GatewayResult},
    state::GatewayState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use runtime::compute::{
    ComputeRegistryState, ComputeStatus, EnrollRequest, SaveDataSource, SaveTarget, SelectTarget,
};
use std::sync::Arc;

fn registry(state: &GatewayState) -> GatewayResult<Arc<runtime::compute::ComputeRegistry>> {
    state.infra.compute.clone().ok_or_else(|| {
        GatewayError::Status(
            StatusCode::CONFLICT,
            "ordinary compute selection is unavailable in formal deployments".into(),
        )
    })
}

/// Read-only inspection; POST carries bounded explicit workflow requirements.
#[utoipa::path(post,path="/api/v1/compute/readiness",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn readiness(
    State(state): State<GatewayState>,
    Json(request): Json<runtime::compute::ReadinessRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    registry(&state)?
        .readiness(&request, &state.infra.engine_manager.list_nodes())
        .map(Json)
        .map_err(compute_error)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageQuery {
    target_id: String,
    target_revision: u64,
    reference: String,
}
#[utoipa::path(
    get, path="/api/v1/compute/image-evidence", tag="compute",
    params(("target_id"=String,Query),("target_revision"=u64,Query),("reference"=String,Query)),
    responses((status=200,body=Object),(status=409,description="Resident mapping unavailable"))
)]
pub(crate) async fn image_evidence(
    State(state): State<GatewayState>,
    Query(query): Query<ImageQuery>,
) -> GatewayResult<Json<runtime::compute::ComputeImageEvidence>> {
    registry(&state)?
        .image_evidence(&query.target_id, query.target_revision, &query.reference)
        .map(Json)
        .map_err(|e| GatewayError::Status(StatusCode::CONFLICT, e))
}
#[utoipa::path(get,path="/api/v1/compute",tag="compute",responses((status=200,body=Object)))]
pub(crate) async fn status(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ComputeStatus>> {
    registry(&state)?.status().map(Json).map_err(compute_error)
}
/// One-shot composite enrollment from minimal operator input.
#[utoipa::path(post,path="/api/v1/compute/enroll",tag="compute",request_body=Object,responses((status=200,body=Object),(status=409,description="enrollment step failed")))]
pub(crate) async fn enroll(
    State(state): State<GatewayState>,
    Json(request): Json<EnrollRequest>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    registry(&state)?
        .enroll(request)
        .await
        .map(Json)
        .map_err(compute_error)
}

#[utoipa::path(put,path="/api/v1/compute/targets",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn target(
    State(gateway): State<GatewayState>,
    Json(request): Json<SaveTarget>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    registry(&state)?
        .save_target(request)
        .map(|registry_state| {
            changed(&gateway, &registry_state);
            Json(registry_state)
        })
        .map_err(compute_error)
}
#[utoipa::path(put,path="/api/v1/compute/policy",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn select(
    State(gateway): State<GatewayState>,
    Json(request): Json<SelectTarget>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    registry(&state)?
        .select(request)
        .map(|registry_state| {
            changed(&gateway, &registry_state);
            Json(registry_state)
        })
        .map_err(compute_error)
}
#[utoipa::path(post,path="/api/v1/compute/data-sources",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn source(
    State(gateway): State<GatewayState>,
    Json(request): Json<SaveDataSource>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    registry(&state)?
        .add_source(request)
        .await
        .map(|registry_state| {
            changed(&gateway, &registry_state);
            Json(registry_state)
        })
        .map_err(compute_error)
}
#[utoipa::path(post,path="/api/v1/compute/targets/{id}/check",tag="compute",params(("id"=String,Path)),responses((status=200,body=Object)))]
pub(crate) async fn check(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> GatewayResult<Json<serde_json::Value>> {
    registry(&state)?
        .check_target(&id)
        .await
        .map(Json)
        .map_err(compute_error)
}

fn changed(gateway: &GatewayState, state: &ComputeRegistryState) {
    gateway.hub.publish(crate::hub::EventKind::Notice(
        crate::proto::GatewayNotice::ComputeChanged {
            registry_revision: state.revision,
            target_id: state.policy.target_id.clone(),
            target_revision: state.policy.target_revision,
        },
    ));
}

fn compute_error(message: String) -> GatewayError {
    if message.starts_with("stale compute revision") {
        GatewayError::Status(StatusCode::CONFLICT, message)
    } else if message.starts_with("compute persistence outcome uncertain") {
        GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, message)
    } else {
        GatewayError::Message(message)
    }
}
