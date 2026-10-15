use axum::{Json, extract::State};

use crate::{proto::ObservationView, server::GatewayState};

#[utoipa::path(
    get,
    path = "/api/v1/evolution/observations",
    tag = "evolution",
    responses((status = 200, body = [ObservationView]))
)]
pub(crate) async fn list_observations(
    State(state): State<GatewayState>,
) -> Json<Vec<ObservationView>> {
    let mut observations = state.infra.skills.observations().await;
    observations.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    Json(
        observations
            .into_iter()
            .map(|observation| ObservationView {
                kind: observation.kind_label().to_string(),
                source: observation.source.into_str().to_string(),
                id: observation.id,
                created_at: observation.created_at,
                summary: observation.summary,
                body: observation.body,
                node_kind: observation.node_kind,
                error: observation.error,
            })
            .collect(),
    )
}

pub(crate) async fn clustering(State(state): State<GatewayState>) -> Json<Vec<ObservationView>> {
    let evo_handle = &state.infra.evolution_handle;
    evo_handle.trigger_clustering();
    todo!()
}
>>>>>>> 936c336 (refactor: remove observation router)
