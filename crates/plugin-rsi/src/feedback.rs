//! Plugin-specific mapping from shared evolution evidence to plugin demand.

use evolution_core::observation::{
    Observation, ObservationInput, ObservationKind, ObservationSource,
};

use crate::{Error, RequestIntent, RequestRecord, RequestSource, RequestStatus, Result};

/// A plugin request promoted from one canonical observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRequest {
    /// The shared observation retained for skill distillation.
    pub observation: Observation,
    /// The plugin demand backed by that observation.
    pub request: RequestRecord,
}

fn request_source(source: ObservationSource) -> RequestSource {
    match source {
        ObservationSource::Agent => RequestSource::Agent,
        ObservationSource::Eval => RequestSource::Eval,
        ObservationSource::WorkflowRun => RequestSource::WorkflowRun,
        ObservationSource::Cli => RequestSource::User,
    }
}

pub(crate) fn observation_request(
    requests: &crate::RequestStore,
    observations: &evolution_core::ObservationStore,
    observation_id: &str,
    intent: RequestIntent,
    plugin_name: Option<&str>,
) -> Result<ObservationRequest> {
    let observation = observations
        .list()
        .into_iter()
        .find(|observation| observation.id == observation_id)
        .ok_or_else(|| Error::InvalidRequest(format!("unknown observation {observation_id:?}")))?;
    let request = requests.record(RequestRecord {
        id: String::new(),
        created_at: 0,
        source: request_source(observation.source),
        intent,
        summary: observation.summary.clone(),
        body: observation.body.clone(),
        plugin_name: plugin_name.map(str::to_string),
        evidence_ids: vec![observation.id.clone()],
        status: RequestStatus::Open,
    })?;
    Ok(ObservationRequest {
        observation,
        request,
    })
}

pub(crate) fn default_request_intent(kind: ObservationKind) -> RequestIntent {
    match kind {
        ObservationKind::Failure => RequestIntent::FixNode,
        ObservationKind::Recipe | ObservationKind::Caveat => RequestIntent::OptimizeNode,
    }
}
