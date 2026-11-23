//! Operator compute/data configuration, independent from conversation state.
use super::{
    error::{GatewayError, GatewayResult},
    state::GatewayState,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, KeepAliveStream, Sse},
    routing::{delete, get, post, put},
};
use runtime::compute::{
    ComputeRegistryState, ComputeStatus, EnrollProgress, EnrollRequest, SaveDataSource, SaveTarget,
    SaveTargetDataSource, SelectTarget,
};
use std::{convert::Infallible, future::Future};
use tokio_stream::wrappers::ReceiverStream;

pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/compute", get(status))
        .route("/compute/readiness", post(readiness))
        .route("/compute/enroll", post(enroll))
        .route("/compute/enroll/stream", post(enroll_stream))
        .route("/compute/repair-worker", post(repair_worker))
        .route("/compute/repair-worker/stream", post(repair_worker_stream))
        .route("/compute/targets", put(target))
        .route("/compute/target-data-source", put(target_data_source))
        .route("/compute/targets/{id}", delete(delete_target))
        .route("/compute/targets/{id}/check", post(check))
        .route("/compute/policy", put(select))
        .route("/compute/data-sources", post(source))
        .route("/compute/image-evidence", get(image_evidence))
}

/// Read-only inspection; POST carries bounded explicit workflow requirements.
#[utoipa::path(post,path="/api/v1/compute/readiness",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn readiness(
    State(state): State<GatewayState>,
    Json(request): Json<runtime::compute::ReadinessRequest>,
) -> GatewayResult<Json<serde_json::Value>> {
    state
        .infra
        .compute
        .readiness(&request, &state.infra.engine_manager.list_nodes())
        .map(Json)
        .map_err(compute_error)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageQuery {
    target_id: String,
    reference: String,
}
#[utoipa::path(
    get, path="/api/v1/compute/image-evidence", tag="compute",
    params(("target_id"=String,Query),("reference"=String,Query)),
    responses((status=200,body=Object),(status=409,description="Resident mapping unavailable"))
)]
pub(crate) async fn image_evidence(
    State(state): State<GatewayState>,
    Query(query): Query<ImageQuery>,
) -> GatewayResult<Json<runtime::compute::ComputeImageEvidence>> {
    state
        .infra
        .compute
        .image_evidence(&query.target_id, &query.reference)
        .map(Json)
        .map_err(|e| GatewayError::Status(StatusCode::CONFLICT, e))
}
#[utoipa::path(get,path="/api/v1/compute",tag="compute",responses((status=200,body=Object)))]
pub(crate) async fn status(
    State(state): State<GatewayState>,
) -> GatewayResult<Json<ComputeStatus>> {
    state
        .infra
        .compute
        .status()
        .map(Json)
        .map_err(compute_error)
}
/// One-shot composite enrollment from minimal operator input.
#[utoipa::path(post,path="/api/v1/compute/enroll",tag="compute",request_body=Object,responses((status=200,body=Object),(status=400,description="Enrollment step failed")))]
pub(crate) async fn enroll(
    State(state): State<GatewayState>,
    Json(request): Json<EnrollRequest>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    state
        .infra
        .compute
        .enroll(request)
        .await
        .map(Json)
        .map_err(compute_error)
}

#[utoipa::path(
    post, path = "/api/v1/compute/repair-worker", tag = "compute", request_body = Object,
    responses((status = 200, body = Object), (status = 400, description = "Worker repair failed"))
)]
pub(crate) async fn repair_worker(
    State(gateway): State<GatewayState>,
    Json(request): Json<runtime::compute::RepairWorkerRequest>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    let result = gateway
        .infra
        .compute
        .repair_worker(request)
        .await
        .map_err(compute_error)?;
    changed(&gateway, &result);
    Ok(Json(result))
}

/// The operation keeps running if the viewer disconnects.
type ProgressStream = Sse<KeepAliveStream<ReceiverStream<Result<Event, Infallible>>>>;

#[utoipa::path(
    post, path = "/api/v1/compute/enroll/stream", tag = "compute", request_body = Object,
    responses((status = 200, description = "Enrollment progress and terminal result or error", body = String, content_type = "text/event-stream"))
)]
pub(crate) async fn enroll_stream(
    State(state): State<GatewayState>,
    Json(request): Json<EnrollRequest>,
) -> GatewayResult<ProgressStream> {
    let registry = state.infra.compute.clone();
    Ok(stream_compute_progress(state, move |progress| async move {
        registry.enroll_with_progress(request, progress).await
    }))
}

#[utoipa::path(
    post, path = "/api/v1/compute/repair-worker/stream", tag = "compute", request_body = Object,
    responses((status = 200, description = "Repair progress and terminal result or error", body = String, content_type = "text/event-stream"))
)]
pub(crate) async fn repair_worker_stream(
    State(state): State<GatewayState>,
    Json(request): Json<runtime::compute::RepairWorkerRequest>,
) -> GatewayResult<ProgressStream> {
    let registry = state.infra.compute.clone();
    Ok(stream_compute_progress(state, move |progress| async move {
        registry
            .repair_worker_with_progress(request, progress)
            .await
    }))
}

fn stream_compute_progress<F>(
    state: GatewayState,
    operation: impl FnOnce(EnrollProgress) -> F + Send + 'static,
) -> ProgressStream
where
    F: Future<Output = Result<ComputeRegistryState, String>> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::mpsc::channel(128);
    let updates = sender.clone();
    let progress = EnrollProgress::with_events(move |event| {
        // Bound memory for slow readers and let the terminal event wait for space.
        let mut value = serde_json::to_value(event).expect("serialize enrollment progress");
        value["type"] = serde_json::json!("progress");
        let _ = updates.try_send(Ok(Event::default().data(value.to_string())));
    });
    tokio::spawn(async move {
        let result = match operation(progress).await {
            Ok(result) => {
                changed(&state, &result);
                serde_json::json!({"type":"result", "state":result})
            }
            Err(error) => serde_json::json!({"type":"error", "message":error}),
        };
        let _ = sender
            .send(Ok(Event::default().data(result.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(receiver)).keep_alive(KeepAlive::default())
}

#[utoipa::path(put,path="/api/v1/compute/targets",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn target(
    State(gateway): State<GatewayState>,
    Json(request): Json<SaveTarget>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    gateway
        .infra
        .compute
        .save_target(request)
        .map(|registry_state| {
            changed(&gateway, &registry_state);
            Json(registry_state)
        })
        .map_err(compute_error)
}

#[utoipa::path(put,path="/api/v1/compute/target-data-source",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn target_data_source(
    State(gateway): State<GatewayState>,
    Json(request): Json<SaveTargetDataSource>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    let snapshot = gateway
        .infra
        .compute
        .save_target_data_source(request)
        .await
        .map_err(compute_error)?;
    changed(&gateway, &snapshot);
    Ok(Json(snapshot))
}

#[utoipa::path(
    delete, path = "/api/v1/compute/targets/{id}", tag = "compute",
    params(("id" = String, Path)),
    responses((status = 200, body = Object), (status = 400, description = "Target deletion failed"))
)]
pub(crate) async fn delete_target(
    State(gateway): State<GatewayState>,
    Path(id): Path<String>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    gateway
        .infra
        .compute
        .delete_target(&id)
        .map(|snapshot| {
            changed(&gateway, &snapshot);
            Json(snapshot)
        })
        .map_err(compute_error)
}
#[utoipa::path(put,path="/api/v1/compute/policy",tag="compute",request_body=Object,responses((status=200,body=Object)))]
pub(crate) async fn select(
    State(gateway): State<GatewayState>,
    Json(request): Json<SelectTarget>,
) -> GatewayResult<Json<ComputeRegistryState>> {
    gateway
        .infra
        .compute
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
    gateway
        .infra
        .compute
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
    state
        .infra
        .compute
        .check_target(&id)
        .await
        .map(Json)
        .map_err(compute_error)
}

pub(super) fn changed(gateway: &GatewayState, state: &ComputeRegistryState) {
    gateway.hub.publish(crate::hub::EventKind::Notice(
        crate::proto::GatewayNotice::ComputeChanged {
            target_id: state.policy.target_id.clone(),
        },
    ));
}

fn compute_error(message: String) -> GatewayError {
    if message.starts_with("compute persistence outcome uncertain") {
        GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, message)
    } else {
        GatewayError::Message(message)
    }
}
