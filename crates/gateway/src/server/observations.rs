use axum::{Json, extract::State};

use crate::{
    proto::{ClusterMemberView, ClusterView, ObservationView},
    server::GatewayState,
};

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

#[utoipa::path(
    get,
    path = "/api/v1/evolution/clusters",
    tag = "evolution",
    responses((status = 200, body = [ClusterView]))
)]
/// Surfaced clusters, hydrated for display: member ids resolve to
/// summaries through the observation ledger. Only clusters at
/// [`evolution_core::cluster::MIN_CLUSTER`] members or more are
/// returned — smaller ones are not patterns yet. The pass itself is
/// triggered by the evolution worker on record; this endpoint is a
/// pure read of what it persisted.
pub(crate) async fn list_clusters(State(state): State<GatewayState>) -> Json<Vec<ClusterView>> {
    use std::collections::HashMap;

    let observations: HashMap<_, _> = state
        .infra
        .skills
        .observations()
        .await
        .into_iter()
        .map(|o| (o.id.clone(), o))
        .collect();
    let mut clusters = state.infra.skills.clusters().await;
    clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.id.cmp(&b.id)));
    Json(
        clusters
            .into_iter()
            .filter(|c| c.members.len() >= evolution_core::cluster::MIN_CLUSTER)
            .map(|cluster| {
                // partition_key encodes `kind \u{1f} node_kind`; both
                // halves exist by construction when the key exists.
                let (kind, node_kind) = cluster
                    .partition_key
                    .split_once('\u{1f}')
                    .map(|(k, n)| (k.to_string(), n.to_string()))
                    .unwrap_or_else(|| (cluster.partition_key.clone(), String::new()));
                let mut members: Vec<_> = cluster
                    .members
                    .iter()
                    .filter_map(|id| {
                        observations.get(id).map(|o| ClusterMemberView {
                            id: o.id.clone(),
                            created_at: o.created_at,
                            summary: o.summary.clone(),
                        })
                    })
                    .collect();
                members.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
                ClusterView {
                    id: cluster.id,
                    kind,
                    node_kind,
                    model_signature: cluster.model_signature,
                    members,
                }
            })
            .collect(),
    )
}
