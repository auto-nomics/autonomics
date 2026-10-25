//! Frontend observability for the resident knowledge workers.

use axum::Json;
use axum::extract::State;

use super::state::GatewayState;
use crate::proto::KmsWorkerStatusView;

#[utoipa::path(
    get,
    path = "/api/v1/kms/workers",
    tag = "kms",
    responses((status = 200, body = [KmsWorkerStatusView]))
)]
pub(crate) async fn get_worker_statuses(
    State(state): State<GatewayState>,
) -> Json<Vec<KmsWorkerStatusView>> {
    Json(
        state
            .infra
            .kms
            .worker_statuses()
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}
