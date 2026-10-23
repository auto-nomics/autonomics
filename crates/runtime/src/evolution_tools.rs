//! Unified evidence capture for skill self-evolution.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use evolution_core::{
    ObservationInput, ObservationKind, ObservationSource, handle::EvolutionHandle,
};
use serde_json::json;

/// Record one durable evolution observation for the skill-evolution worker.
#[tool(
    name = "evo_observe",
    description = "Record one durable failure, recipe, or caveat for skill self-evolution. \
                   Plugin development is initiated explicitly with plugin_create or plugin_fork."
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
    /// The skill-evolution control handle, when the service runs.
    control: Option<skills::evolution::SkillControlHandle>,
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
        let observation = self
            .handle
            .record_observation(observation)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(std::io::Error::other(error.to_string())),
            })?;

        // Event-driven evolution: nudge the worker so evidence distills within
        // the quiet window. This is best-effort; the timer sweep catches up.
        if let Some(handle) = self.control.as_ref()
            && !handle.trigger(skills::evolution::EvolutionTrigger::Observation)
        {
            tracing::debug!(
                "evo_observe: evolution channel full or worker gone; the timer sweep will catch up"
            );
        }

        Ok(ToolResult::success_json(json!({
            "observation_id": observation.id,
            "kind": observation.kind_label(),
            "audience": "skill",
            "skill_distillation": true,
        })))
    }
}

/// Build the host-owned unified observation tool.
pub fn evo_observe_registration(
    handle: Arc<EvolutionHandle>,
    control: Option<skills::evolution::SkillControlHandle>,
) -> ToolRegistration {
    ToolRegistration::from(EvoObserveTool { handle, control })
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
            control: None,
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
            control: None,
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

    #[tokio::test]
    async fn all_observations_feed_skill_evolution() {
        let state = tempfile::tempdir().unwrap();
        let (_skills, hub) = skills_with_hub(state.path());
        let tool = EvoObserveTool {
            handle: handle_for(&hub),
            control: None,
        };

        for (summary, kind, node_kind) in [
            ("Generic SQL recipe", "recipe", "sql"),
            ("Owned adapter failure", "failure", "demo/demo_adapter"),
        ] {
            let result = tool
                .run(EvoObserveInput {
                    summary: summary.into(),
                    body: format!("{summary}: reusable evidence."),
                    kind: Some(kind.into()),
                    node_kind: Some(node_kind.into()),
                    error: Some("adapter rejected input".into()),
                })
                .await
                .unwrap();
            let ToolResultContent::Json(value) = result.content else {
                panic!("evo_observe must return JSON");
            };
            assert_eq!(value["audience"], "skill", "{value}");
            assert_eq!(value["skill_distillation"], true, "{value}");
            assert!(value["observation_id"].as_str().is_some(), "{value}");
        }

        assert_eq!(hub.storage().list().await.len(), 2);
    }

    #[tokio::test]
    async fn recorded_observations_trigger_evolution_cycles() {
        let state = tempfile::tempdir().unwrap();
        let (skills, hub) = skills_with_hub(state.path());
        let control = skills::evolution::start(
            skills.clone(),
            skills::evolution::EvolutionOptions {
                quiet_window: std::time::Duration::from_millis(50),
                timer: None,
                ..Default::default()
            },
        );
        let mut reports = control.subscribe_reports();
        let tool = EvoObserveTool {
            handle: handle_for(&hub),
            control: Some(control),
        };

        tool.run(EvoObserveInput {
            summary: "generic recipe".into(),
            body: "generic procedure".into(),
            kind: Some("recipe".into()),
            node_kind: Some("generic_node".into()),
            error: None,
        })
        .await
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_observation_trigger = false;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match tokio::time::timeout(remaining, reports.recv()).await {
                Ok(Ok(report)) if report.triggers.contains(&"observation") => {
                    saw_observation_trigger = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(
            saw_observation_trigger,
            "no evolution cycle carried the observation trigger"
        );
    }
}
