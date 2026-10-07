//! Unified evidence capture for skill and plugin self-evolution.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration, ToolResult};
use agentik_proc::tool;
use async_trait::async_trait;
use plugin_rsi::RsiInfra;
use serde_json::json;
use skills::{ObservationInput, ObservationKind, ObservationSource};

/// Record one durable evolution observation and let the host route it.
#[tool(
    name = "evo_observe",
    description = "Record one durable failure, recipe, or caveat for self-evolution. \
                   The host automatically routes the evidence to skill evolution, plugin \
                   evolution, both, or triage from its node kind and ownership. Do not \
                   choose a route in the input."
)]
pub struct EvoObserveInput {
    /// One line a future search will find; name the component or interface.
    pub summary: String,
    /// The reusable pattern, fix, or condition. Not a run transcript.
    pub body: String,
    /// Observation kind: failure, recipe, or caveat.
    pub kind: Option<String>,
    /// DAG node address (`plugin/node`) or legacy bare kind. This is the
    /// primary automatic routing anchor.
    pub node_kind: Option<String>,
    /// Exact error text for failure observations.
    pub error: Option<String>,
}

pub struct EvoObserveTool {
    infra: RsiInfra,
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
        let routing = self
            .infra
            .record_and_route_observation(observation, None, None)
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(std::io::Error::other(error.to_string())),
            })?;
        let request_id = routing
            .request
            .as_ref()
            .map(|request| request.request.id.clone());
        Ok(ToolResult::success_json(json!({
            "observation_id": routing.observation.id,
            "kind": routing.observation.kind_label(),
            "audience": routing.route.audience,
            "route_id": routing.route.id,
            "route_reason": routing.decision_reason,
            "plugin_name": routing.route.plugin_name,
            "plugin_request_id": request_id,
            "skill_distillation": matches!(
                routing.route.audience,
                plugin_rsi::ObservationAudience::Skill
                    | plugin_rsi::ObservationAudience::Both
            ),
        })))
    }
}

/// Build the host-owned unified observation tool.
pub fn evo_observe_registration(infra: RsiInfra) -> ToolRegistration {
    ToolRegistration::from(EvoObserveTool { infra })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;
    use agentik_sdk::types::ToolResultContent;
    use plugin_rsi::{
        Environment, EnvironmentCatalog, GhPublisher, GhPublisherConfig, PluginStore,
        RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore,
    };
    use std::sync::Arc;

    const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

    fn catalog() -> EnvironmentCatalog {
        let mut environments = EnvironmentCatalog::default();
        environments.insert(
            "alpine",
            Environment {
                reference: ENVIRONMENT_REFERENCE.into(),
                interpreters: vec!["sh".into()],
            },
        );
        environments
    }

    fn infra_with_skills(state: &std::path::Path) -> (RsiInfra, Arc<skills::SkillManager>) {
        let skills = skills::SkillManager::init(skills::SkillManager::new(state));
        let environments = catalog();
        let publisher = Arc::new(GhPublisher::new(GhPublisherConfig {
            enabled: false,
            ..Default::default()
        }));
        let infra = RsiInfra::open(
            state,
            "main",
            "Test",
            "test@example.com",
            skills.clone(),
            environments,
            publisher.clone(),
            publisher,
        )
        .unwrap();
        (infra, skills)
    }

    fn infra(state: &std::path::Path) -> RsiInfra {
        infra_with_skills(state).0
    }

    fn create_plugin_with_node(state: &std::path::Path) {
        let requests = RequestStore::open(state);
        let request = requests
            .record(RequestRecord {
                id: String::new(),
                created_at: 0,
                source: RequestSource::User,
                intent: RequestIntent::NewNode,
                summary: "Create routed plugin".into(),
                body: "Create a node used by unified observation routing.".into(),
                plugin_name: Some("evo-plugin".into()),
                evidence_ids: Vec::new(),
                status: RequestStatus::Open,
            })
            .unwrap();
        let store = PluginStore::open(state, "main", "Test", "test@example.com");
        store
            .create(
                "evo-plugin",
                "alpine",
                std::slice::from_ref(&request.id),
                &request.body,
                &requests,
                &catalog(),
            )
            .unwrap();

        let workspace = state.join("plugins/evo-plugin");
        std::fs::create_dir_all(workspace.join("scripts")).unwrap();
        std::fs::write(workspace.join("scripts/adapter.sh"), "#!/bin/sh\nset -eu\n").unwrap();
        let manifest_path = workspace.join("manifest.toml");
        let mut manifest: container_plugin::manifest::PluginManifest =
            toml::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let node: container_plugin::node_definition::NodeDefinition =
            serde_json::from_value(serde_json::json!({
                "kind": "evo_adapter",
                "desc": "Route observations to its owning plugin",
                "doc": "Input is copied to output.",
                "ports": {
                    "inputs": [{ "type": "file", "label": "input" }],
                    "outputs": [{ "path": "result.txt", "format": "txt" }]
                },
                "command": {
                    "interpreter": "sh",
                    "argv": [],
                    "script_file": "scripts/adapter.sh",
                    "env": {},
                    "files": {}
                }
            }))
            .unwrap();
        manifest.nodes.push(node);
        std::fs::write(&manifest_path, toml::to_string_pretty(&manifest).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn unowned_recipes_route_to_skills_and_unowned_failures_wait_for_triage() {
        let state = tempfile::tempdir().unwrap();
        let tool = EvoObserveTool {
            infra: infra(state.path()),
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
        assert!(recipe.text_content().contains("skill"));

        let failure = tool
            .run(EvoObserveInput {
                summary: "Unknown node failed".into(),
                body: "This failure needs triage.".into(),
                kind: Some("failure".into()),
                node_kind: Some("unknown_node".into()),
                error: Some("node not found".into()),
            })
            .await
            .unwrap();
        assert!(failure.text_content().contains("unassigned"));
    }

    #[tokio::test]
    async fn owned_node_failures_route_to_plugins_and_do_not_distill_into_skills() {
        let state = tempfile::tempdir().unwrap();
        create_plugin_with_node(state.path());
        let (infra, skills) = infra_with_skills(state.path());
        let tool = EvoObserveTool { infra };

        let both = tool
            .run(EvoObserveInput {
                summary: "Owned adapter recipe".into(),
                body: "Use the adapter contract directly.".into(),
                kind: Some("recipe".into()),
                node_kind: Some("evo_adapter".into()),
                error: None,
            })
            .await
            .unwrap();
        let ToolResultContent::Json(value) = both.content else {
            panic!("evo_observe must return JSON");
        };
        assert_eq!(value["audience"], "both", "{value}");
        assert_eq!(value["skill_distillation"], true, "{value}");
        assert!(value["plugin_request_id"].as_str().is_some(), "{value}");

        for index in 0..3 {
            let result = tool
                .run(EvoObserveInput {
                    summary: format!("Owned adapter failure {index}"),
                    body: format!("The adapter should recover from case {index}."),
                    kind: Some("failure".into()),
                    node_kind: Some("evo_adapter".into()),
                    error: Some("adapter rejected input".into()),
                })
                .await
                .unwrap();
            let ToolResultContent::Json(value) = result.content else {
                panic!("evo_observe must return JSON");
            };
            assert_eq!(value["audience"], "plugin", "{value}");
            assert_eq!(value["plugin_name"], "evo-plugin", "{value}");
            assert!(value["plugin_request_id"].as_str().is_some(), "{value}");
            assert_eq!(value["skill_distillation"], false, "{value}");
        }
        skills.distill().unwrap();
        assert!(skills.proposals().list().is_empty());

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
        skills.distill().unwrap();
        let proposals = skills.proposals().list();
        assert_eq!(proposals.len(), 1, "{proposals:?}");
        assert_eq!(proposals[0].name, "generic-node-recipe");
    }
}
