//! Unified evidence capture for skill and plugin self-evolution.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use evolution_core::{
    ObservationInput, ObservationKind, ObservationSource, handle::EvolutionHandle,
};
use serde_json::json;

/// Record one durable evolution observation into the shared ledger.
#[tool(
    name = "evo_observe",
    description = "Record one durable failure, recipe, or caveat for self-evolution. \
                   The observation lands in the shared evidence ledger every \
                   evolution subsystem (skills, plugins) consumes; do not \
                   choose a route in the input."
)]
pub struct EvoObserveInput {
    /// One line a future search will find; name the component or interface.
    pub summary: String,
    /// The reusable pattern, fix, or condition. Not a run transcript.
    pub body: String,
    /// Observation kind: failure, recipe, or caveat.
    pub kind: Option<String>,
    /// DAG node address (`plugin/node`) or legacy bare kind, stored as
    /// the observation's structural anchor for clustering.
    pub node_kind: Option<String>,
    /// Exact error text for failure observations.
    pub error: Option<String>,
}

pub struct EvoObserveTool {
    handle: Arc<EvolutionHandle>,
}

#[async_trait]
impl ToolFunction for EvoObserveTool {
    type Input = EvoObserveInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let summary = input.summary.trim().to_string();
        let body = input.body.trim().to_string();
        if summary.is_empty() || body.is_empty() {
            return Ok(ToolResult::error(
                "evo_observe: 'summary' and 'body' must be non-empty",
            ));
        }
        let kind = match input.kind.as_deref().map(str::trim) {
            None | Some("") | Some("failure") => ObservationKind::Failure,
            Some("recipe") => ObservationKind::Recipe,
            Some("caveat") => ObservationKind::Caveat,
            Some(other) => {
                return Ok(ToolResult::error(format!(
                    "evo_observe: unknown kind {other:?} (use failure, recipe, or caveat)"
                )));
            }
        };
        let observation = ObservationInput {
            kind,
            source: ObservationSource::Agent,
            summary,
            body,
            node_kind: input
                .node_kind
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            error: input
                .error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        };
        let obs = self
            .handle
            .record_observation(observation)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(std::io::Error::other(error.to_string())),
            })?;

        Ok(ToolResult::success_json(json!({
            "observation_id": &obs.id,
            "kind": &obs.kind_label(),
        })))
    }
}

/// Build the host-owned unified observation tool.
pub fn evo_observe_registration(handle: Arc<EvolutionHandle>) -> ToolRegistration {
    ToolRegistration::from(EvoObserveTool { handle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use agentik_sdk::types::ToolResultContent;
    use std::sync::Arc;

    fn skills_with_hub(
        state: &std::path::Path,
    ) -> (
        Arc<skills::SkillManager>,
        Arc<evolution_core::ObservationHub>,
    ) {
        let skills = skills::SkillManager::init(skills::SkillManager::new(state));
        let hub = skills.observation_hub();
        (skills, hub)
    }

    /// Start a worker over the shared hub so `EvoObserveTool`'s
    /// `record_observation` calls land in the same store the skills
    /// manager reads. Without this the tool would record into a parallel
    /// hub and the distillation tests would observe empty proposals.
    fn handle_for(
        hub: &Arc<evolution_core::ObservationHub>,
    ) -> Arc<evolution_core::handle::EvolutionHandle> {
        Arc::new(evolution_core::worker::start(Arc::clone(hub)))
    }

    #[tokio::test]
    async fn recorded_observations_persist_into_the_shared_ledger() {
        let state = tempfile::tempdir().unwrap();
        let (_skills, hub) = skills_with_hub(state.path());
        let tool = EvoObserveTool {
            handle: handle_for(&hub),
        };

        let recipe = tool
            .run(EvoObserveInput {
                summary: "Generic SQL recipe".into(),
                body: "Use lowercase identifiers.".into(),
                kind: Some("recipe".into()),
                node_kind: Some("sql".into()),
                error: None,
            })
            .await
            .unwrap();
        let ToolResultContent::Json(value) = recipe.content else {
            panic!("evo_observe must return JSON");
        };
        let id = value["observation_id"].as_str().expect("id").to_string();
        assert_eq!(value["kind"], "recipe", "{value}");

        let failure = tool
            .run(EvoObserveInput {
                summary: "Unknown node failed".into(),
                body: "This failure needs triage.".into(),
                kind: None,
                node_kind: Some("unknown_node".into()),
                error: Some("node not found".into()),
            })
            .await
            .unwrap();
        let ToolResultContent::Json(failure_value) = failure.content else {
            panic!("evo_observe must return JSON");
        };
        let failure_id = failure_value["observation_id"]
            .as_str()
            .expect("id")
            .to_string();
        assert_eq!(failure_value["kind"], "failure", "{failure_value}");

        let stored = hub.storage().list().await;
        assert!(stored.iter().any(|o| o.id == id));
        assert!(stored.iter().any(|o| o.id == failure_id));
    }

    #[tokio::test]
    async fn recorded_recipes_distill_into_skill_proposals() {
        let state = tempfile::tempdir().unwrap();
        let (skills, hub) = skills_with_hub(state.path());
        let tool = EvoObserveTool {
            handle: handle_for(&hub),
        };

        for index in 0..3 {
            tool.run(EvoObserveInput {
                summary: format!("Generic node recipe {index}"),
                body: format!("Generic procedure {index}."),
                kind: Some("recipe".into()),
                node_kind: Some("generic_node".into()),
                error: None,
            })
            .await
            .unwrap();
        }
        skills.distill().await.unwrap();
        let proposals = skills.proposals().list();
        assert_eq!(proposals.len(), 1, "{proposals:?}");
        // `{node_kind}-{signature}-recipe` with the signature derived
        // from the summary keywords ("generic node recipe") — the
        // kind slug and the keyword slug overlap by design here.
        assert_eq!(proposals[0].name, "generic-node-generic-node-recipe-recipe");
    }
}
