//! Unified evidence capture for skill self-evolution.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use evolution_core::{ObservationHub, ObservationInput, ObservationKind, ObservationSource};
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
    /// DAG node address (`plugin/node`) or legacy bare kind used as metadata.
    pub node_kind: Option<String>,
    /// Exact error text for failure observations.
    pub error: Option<String>,
}

pub struct EvoObserveTool {
    observations: Arc<ObservationHub>,
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
            .observations
            .record(observation)
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
    observations: Arc<ObservationHub>,
    control: Option<skills::evolution::SkillControlHandle>,
) -> ToolRegistration {
    ToolRegistration::from(EvoObserveTool {
        observations,
        control,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use agentik_sdk::types::ToolResultContent;

    fn observations_with_skills(
        state: &std::path::Path,
    ) -> (Arc<ObservationHub>, Arc<skills::SkillManager>) {
        let skills = skills::SkillManager::init(skills::SkillManager::new(state));
        (skills.observation_hub(), skills)
    }

    #[tokio::test]
    async fn all_observations_feed_skill_evolution() {
        let state = tempfile::tempdir().unwrap();
        let (observations, skills) = observations_with_skills(state.path());
        let tool = EvoObserveTool {
            observations,
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

        assert_eq!(skills.observation_hub().storage().list().await.len(), 2);
    }

    #[tokio::test]
    async fn recorded_observations_trigger_evolution_cycles() {
        let state = tempfile::tempdir().unwrap();
        let (observations, skills) = observations_with_skills(state.path());
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
            observations,
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
