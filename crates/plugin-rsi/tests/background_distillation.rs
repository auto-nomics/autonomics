use std::sync::Arc;

mod common;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, GitRepo, MergeOutcome, PluginDistiller,
    PluginPublisher, PluginPullRequestPublisher, PluginStatus, PublishOutcome, PullRequestOutcome,
    RequestIntent, RequestRecord, RequestSource, RequestStatus, RsiInfra,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";
const REMOTE: &str = "git@github.com:auto-nomics/distilled-plugin-plugin.git";

#[derive(Default)]
struct FakePublisher;

impl PluginPublisher for FakePublisher {
    fn publish_plugin(
        &self,
        _plugin_name: &str,
        repo: &GitRepo,
    ) -> plugin_rsi::Result<PublishOutcome> {
        if repo.remote_url("origin").unwrap().is_none() {
            repo.add_remote("origin", REMOTE)?;
        }
        Ok(PublishOutcome {
            remote: REMOTE.into(),
            commit: repo.head()?,
            repository_created: true,
        })
    }
}

#[derive(Default)]
struct FakePullRequestPublisher;

impl PluginPullRequestPublisher for FakePullRequestPublisher {
    fn open_pull_request(
        &self,
        _plugin_name: &str,
        branch: &str,
        repo: &GitRepo,
    ) -> plugin_rsi::Result<PullRequestOutcome> {
        repo.switch_new_branch(branch)?;
        let commit = repo.head()?;
        Ok(PullRequestOutcome {
            remote: REMOTE.into(),
            commit,
            branch: branch.into(),
            number: 4,
            url: "https://github.com/auto-nomics/distilled-plugin-plugin/pull/4".into(),
        })
    }

    fn merge_pull_request(&self, remote: &str, number: u64) -> plugin_rsi::Result<MergeOutcome> {
        Ok(MergeOutcome {
            remote: remote.into(),
            commit: format!("{number:040}"),
        })
    }
}

struct NoopRegistry;

impl plugin_rsi::PluginRegistryControl for NoopRegistry {
    fn installed_node_kinds(&self) -> plugin_rsi::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn reload_plugin(&self, _plugin_name: &str) -> plugin_rsi::Result<()> {
        Ok(())
    }
}

fn catalog() -> EnvironmentCatalog {
    let mut catalog = EnvironmentCatalog::default();
    catalog.insert(
        "alpine",
        Environment {
            reference: ENVIRONMENT_REFERENCE.into(),
            interpreters: vec!["sh".into()],
        },
    );
    catalog
}

fn node_json() -> Value {
    json!({
        "kind": "distilled_adapter",
        "desc": "Copy one file",
        "doc": "Copies input 0 to output 0.",
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
    })
}

async fn execute(
    tools: &[ToolRegistration],
    name: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    tools
        .iter()
        .find(|tool| tool.definition.name == name)
        .unwrap()
        .implementation
        .execute(input)
        .await
}

#[tokio::test]
async fn local_activation_is_distilled_to_github_without_blocking_the_agent() {
    let state = tempfile::tempdir().unwrap();
    common::configure_plugin_vfs(state.path());
    let skills = skills::SkillManager::init(skills::SkillManager::new(state.path()));
    let publisher = Arc::new(FakePublisher);
    let infra = RsiInfra::open(
        state.path(),
        "main",
        "Autonomics RSI",
        "rsi@example.com",
        skills,
        catalog(),
        publisher.clone(),
        Arc::new(FakePullRequestPublisher),
    )
    .unwrap();
    infra.configure_registry(Arc::new(NoopRegistry));

    let request = infra
        .requests()
        .record(RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::User,
            intent: RequestIntent::NewNode,
            summary: "Create a locally active adapter".into(),
            body: "Copy one input file.".into(),
            plugin_name: Some("distilled-plugin".into()),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        })
        .unwrap();
    let _operator = infra.create_plugin(request.clone(), "alpine").unwrap();
    let profile = AgentProfile::new("distillation-agent").unwrap();
    let tools = profile.tool_registrations();
    execute(
        &tools,
        "plugin_node_create",
        json!({
            "plugin_path": "/plugins/dev/distilled-plugin",
            "node": node_json()
        }),
    )
    .await
    .unwrap();
    execute(
        &tools,
        "plugin_node_write_script",
        json!({
            "plugin_path": "/plugins/dev/distilled-plugin",
            "node_kind": "distilled_adapter",
            "contents": "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n"
        }),
    )
    .await
    .unwrap();
    execute(
        &tools,
        "plugin_workspace_write",
        json!({
            "plugin_path": "/plugins/dev/distilled-plugin",
            "path": "README.md",
            "contents": "# distilled-plugin\n\nA deterministic adapter.\n"
        }),
    )
    .await
    .unwrap();

    let distiller = PluginDistiller::new(infra.clone());
    let local = infra
        .validate_and_activate_local("distilled-plugin")
        .unwrap();
    assert!(matches!(
        local,
        plugin_rsi::LocalActivationOutcome::Activated(_, _)
    ));
    let pending = distiller.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].plugin_name, "distilled-plugin");

    let report = distiller.run_once();
    assert_eq!(report.completed, 1, "{report:?}");
    assert!(infra.pending_distillation().unwrap().is_empty());
    let store = infra.store();
    let manifest = store.develop("distilled-plugin").unwrap().unwrap();
    assert_eq!(manifest.status(), PluginStatus::Installed);
    assert_eq!(
        infra.requests().find(&request.id).unwrap().unwrap().status,
        RequestStatus::Consumed
    );
}
