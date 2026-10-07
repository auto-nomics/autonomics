use std::sync::Mutex;

mod common;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
use plugin_rsi::PluginStateLayout;
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, GitRepo, MergeOutcome, PluginLifecycle,
    PluginPublisher, PluginPullRequestPublisher, PluginStatus, PluginStore, PublishOutcome,
    PullRequestOutcome, RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore,
    ValidationOutcome,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";
const REMOTE: &str = "git@github.com:auto-nomics/unified-plugin-plugin.git";

#[derive(Default)]
struct FakePublisher {
    calls: Mutex<Vec<String>>,
}

impl PluginPublisher for FakePublisher {
    fn publish_plugin(
        &self,
        plugin_name: &str,
        repo: &GitRepo,
    ) -> plugin_rsi::Result<PublishOutcome> {
        self.calls.lock().unwrap().push(plugin_name.to_string());
        if repo.remote_url("origin").unwrap().is_none() {
            repo.add_remote("origin", REMOTE)?;
        } else if repo.remote_url("origin").unwrap().as_deref() != Some(REMOTE) {
            return Err(plugin_rsi::Error::Validation("wrong origin".into()));
        }
        Ok(PublishOutcome {
            remote: REMOTE.into(),
            commit: repo.head()?,
            repository_created: true,
        })
    }
}

#[derive(Default)]
struct FakePullRequestPublisher {
    opened_commit: Mutex<Option<String>>,
}

impl PluginPullRequestPublisher for FakePullRequestPublisher {
    fn open_pull_request(
        &self,
        _plugin_name: &str,
        branch: &str,
        repo: &GitRepo,
    ) -> plugin_rsi::Result<PullRequestOutcome> {
        let commit = repo.head()?;
        repo.switch_new_branch(branch)?;
        *self.opened_commit.lock().unwrap() = Some(commit.clone());
        Ok(PullRequestOutcome {
            remote: REMOTE.into(),
            commit,
            branch: branch.into(),
            number: 9,
            url: "https://github.com/auto-nomics/unified-plugin-plugin/pull/9".to_string(),
        })
    }

    fn merge_pull_request(&self, remote: &str, number: u64) -> plugin_rsi::Result<MergeOutcome> {
        assert_eq!(remote, REMOTE);
        assert_eq!(number, 9);
        let commit = self
            .opened_commit
            .lock()
            .unwrap()
            .clone()
            .expect("PR must be opened before merge");
        Ok(MergeOutcome {
            remote: remote.into(),
            commit,
        })
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

fn request(summary: &str, body: &str, intent: RequestIntent) -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent,
        summary: summary.into(),
        body: body.into(),
        plugin_name: Some("unified-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn node_json() -> Value {
    json!({
        "kind": "unified_adapter",
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
async fn plugin_is_developed_published_updated_and_installed_in_one_workspace() {
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let first_request = requests
        .record(request(
            "Create a deterministic adapter",
            "Copy the input file to the declared output.",
            RequestIntent::NewNode,
        ))
        .unwrap();
    common::configure_plugin_vfs(state.path());
    let layout = PluginStateLayout::v2(state.path());
    let store =
        PluginStore::open_with_layout(layout.clone(), "main", "Autonomics RSI", "rsi@example.com");
    let mut operator = store
        .create(
            "unified-plugin",
            "alpine",
            std::slice::from_ref(&first_request.id),
            "Create the unified lifecycle plugin.",
            &requests,
            &catalog(),
        )
        .unwrap();

    let profile = AgentProfile::new("unified-plugin-agent").unwrap();
    let tools = profile.tool_registrations();
    execute(
        &tools,
        "plugin_node_create",
        json!({
            "plugin_path": "/plugins/dev/unified-plugin",
            "node": node_json()
        }),
    )
    .await
    .unwrap();
    operator
        .workspace()
        .write_text(
            "scripts/adapter.sh",
            "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    operator
        .workspace()
        .write_text(
            "README.md",
            "# unified-plugin\n\nA deterministic adapter.\n",
        )
        .unwrap();

    let validation_catalog = catalog();
    let mut lifecycle = PluginLifecycle::new(&mut operator, &validation_catalog, &[]);
    match lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(report) => assert!(report.passed(), "{report:?}"),
        ValidationOutcome::Passed(_) => panic!("review submission unexpectedly stayed local"),
        ValidationOutcome::NeedsFix(report) => panic!("validation failed: {report:?}"),
    }
    assert_eq!(lifecycle.plugin().status, PluginStatus::PendingReview);
    assert_eq!(
        lifecycle.plugin().lifecycle.request_ids,
        vec![first_request.id.clone()]
    );
    let local_install = store.install_local("unified-plugin").unwrap();
    assert!(local_install.path.join("manifest.toml").is_file());
    assert!(!local_install.path.join(".git").exists());
    let runtime_root = plugin_rsi::PluginStateLayout::v2(state.path()).runtime_root();
    assert!(runtime_root.join("unified-plugin").is_dir());
    assert!(matches!(
        plugin_rsi::read_installed_plugin_source(
            &state.path().join("plugins.toml"),
            "unified-plugin"
        )
        .unwrap(),
        plugin_rsi::InstalledPluginSource::Local(_)
    ));
    lifecycle.review(true).unwrap();
    let publisher = FakePublisher::default();
    lifecycle.publish_reviewed(&publisher).unwrap();
    lifecycle.install(&publisher).unwrap();
    assert_eq!(operator.status(), PluginStatus::Installed);

    let second_request = requests
        .record(request(
            "Reject empty input",
            "Fail when the input file is empty.",
            RequestIntent::OptimizeNode,
        ))
        .unwrap();
    let installed =
        plugin_rsi::read_git_plugin_source(&state.path().join("plugins.toml"), "unified-plugin")
            .unwrap();
    let mut update = store
        .create_update(
            "unified-plugin",
            &[second_request.id],
            "The adapter should reject empty input.",
            &requests,
            &plugin_rsi::InstalledPluginSource::Git(installed.clone()),
            &catalog(),
            &plugin_rsi::GitPluginSourceFetcher,
        )
        .unwrap();
    assert_eq!(update.status(), PluginStatus::Updating);

    update
        .workspace()
        .write_text(
            "scripts/adapter.sh",
            "#!/bin/sh\nset -eu\ntest -s \"$AUTONOMICS_INPUT0\"\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();

    let owned_kinds = ["unified_adapter".to_string()];
    let mut update_lifecycle = PluginLifecycle::new(&mut update, &validation_catalog, &owned_kinds);
    match update_lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(report) => assert!(report.passed(), "{report:?}"),
        ValidationOutcome::Passed(_) => panic!("review submission unexpectedly stayed local"),
        ValidationOutcome::NeedsFix(report) => panic!("update validation failed: {report:?}"),
    }
    update_lifecycle.review(true).unwrap();
    let pull_request_publisher = FakePullRequestPublisher::default();
    update_lifecycle
        .open_update_pull_request(&pull_request_publisher)
        .unwrap();
    update_lifecycle
        .merge_update_pull_request(&pull_request_publisher)
        .unwrap();
    update_lifecycle.install(&publisher).unwrap();
    assert_eq!(update.status(), PluginStatus::Installed);

    let source =
        plugin_rsi::read_git_plugin_source(&state.path().join("plugins.toml"), "unified-plugin")
            .unwrap();
    assert_ne!(source.commit, installed.commit);
    assert_eq!(source.remote, REMOTE);
    assert!(layout.workspace_root().join("unified-plugin").is_dir());
    let rolled_back = store.rollback("unified-plugin").unwrap();
    let plugin_rsi::InstalledPluginSource::Git(rolled_back) = rolled_back else {
        panic!("rollback must return the installed GitHub source");
    };
    assert_eq!(rolled_back.commit, installed.commit);
    assert_eq!(rolled_back.remote, REMOTE);
    update.refresh().unwrap();
    assert_eq!(update.status(), PluginStatus::Installed);
    assert_eq!(
        plugin_rsi::read_git_plugin_source(&state.path().join("plugins.toml"), "unified-plugin")
            .unwrap()
            .commit,
        installed.commit
    );
}
