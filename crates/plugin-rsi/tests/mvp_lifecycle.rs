use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
use async_trait::async_trait;
use container_runtime::{ContainerRunRequest, ContainerRunResult, PodmanConnection};
use plugin_rsi::{
    AgentProfile, Environment, EnvironmentCatalog, GitRepo, InstalledPluginSource, MergeOutcome,
    PluginLifecycle, PluginPublisher, PluginPullRequestPublisher, ProposalStatus, ProposalStore,
    PublishOutcome, PullRequestOutcome, RequestIntent, RequestRecord, RequestSource, RequestStatus,
    RequestStore, ValidationOutcome,
};
use serde_json::{Value, json};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

struct FakeRuntime {
    root: PathBuf,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

#[async_trait]
impl PodmanConnection for FakeRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, container_runtime::ContainerRuntimeError> {
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "adapter self-check completed".into(),
            stderr: String::new(),
        })
    }

    fn workspace_root(&self) -> &std::path::Path {
        &self.root
    }
}

#[derive(Default)]
struct FakePublisher {
    calls: Mutex<Vec<String>>,
}

struct LocalSourceFetcher {
    local_path: PathBuf,
}

impl plugin_rsi::PluginSourceFetcher for LocalSourceFetcher {
    fn fetch(
        &self,
        source: &InstalledPluginSource,
        destination: &std::path::Path,
    ) -> plugin_rsi::Result<()> {
        assert!(source.remote.starts_with("git@github.com:"));
        GitRepo::clone_at(
            destination,
            &self.local_path.to_string_lossy(),
            &source.commit,
            "origin",
        )?;
        std::process::Command::new("git")
            .arg("-C")
            .arg(destination)
            .args(["remote", "set-url", "origin", &source.remote])
            .output()
            .map_err(|source| plugin_rsi::Error::Git {
                command: "remote set-url".into(),
                stderr: source.to_string(),
            })?;
        Ok(())
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
        repo: &plugin_rsi::GitRepo,
    ) -> plugin_rsi::Result<PullRequestOutcome> {
        assert!(branch.starts_with("plugin-rsi/"));
        let commit = repo.head()?;
        *self.opened_commit.lock().unwrap() = Some(commit.clone());
        Ok(PullRequestOutcome {
            remote: "git@github.com:auto-nomics/mvp-plugin-plugin.git".into(),
            commit,
            branch: branch.into(),
            number: 7,
            url: "https://github.com/auto-nomics/mvp-plugin-plugin/pull/7".into(),
        })
    }

    fn merge_pull_request(&self, remote: &str, number: u64) -> plugin_rsi::Result<MergeOutcome> {
        assert_eq!(remote, "git@github.com:auto-nomics/mvp-plugin-plugin.git");
        assert_eq!(number, 7);
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

impl PluginPublisher for FakePublisher {
    fn publish_plugin(
        &self,
        plugin_name: &str,
        repo: &plugin_rsi::GitRepo,
    ) -> plugin_rsi::Result<PublishOutcome> {
        self.calls.lock().unwrap().push(plugin_name.to_string());
        Ok(PublishOutcome {
            remote: format!("git@github.com:auto-nomics/{plugin_name}-plugin.git"),
            commit: repo.head()?,
            repository_created: true,
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

fn request() -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: "Create a deterministic file adapter".into(),
        body: "Copy the input file to the declared output.".into(),
        plugin_name: Some("mvp-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn node_json() -> Value {
    json!({
        "kind": "mvp_adapter",
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
async fn proposal_agent_candidate_review_publish_and_install_complete() {
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let request = requests.record(request()).unwrap();
    let proposals = ProposalStore::open(state.path(), "main", "Autonomics RSI", "rsi@example.com");
    let mut development = proposals
        .create(
            "mvp-plugin",
            &[request.id],
            "The requested adapter is not available yet.",
            &requests,
        )
        .unwrap();
    development.bind_environment("alpine", &catalog()).unwrap();

    let profile = AgentProfile::new("mvp-plugin-agent").unwrap();
    let runtime = Arc::new(FakeRuntime {
        root: state.path().to_path_buf(),
        requests: Mutex::new(Vec::new()),
    });
    profile.bind_plugin(&mut development, "mvp-run-1").unwrap();
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_runtime(runtime.clone())
        .unwrap();
    let tools = profile.tool_registrations();

    execute(&tools, "plugin_node_create", json!({ "node": node_json() }))
        .await
        .unwrap();
    execute(
        &tools,
        "plugin_workspace_write",
        json!({
            "path": "README.md",
            "contents": "# mvp-plugin\n\nA deterministic file adapter.\n"
        }),
    )
    .await
    .unwrap();
    execute(
        &tools,
        "plugin_node_write_script",
        json!({
            "node_kind": "mvp_adapter",
            "contents": "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n"
        }),
    )
    .await
    .unwrap();
    let run = execute(
        &tools,
        "plugin_container_run",
        json!({ "argv": ["sh", "scripts/adapter.sh"] }),
    )
    .await
    .unwrap();
    match run.content {
        ToolResultContent::Json(value) => assert_eq!(value["exit_code"], 0),
        other => panic!("unexpected container result: {other:?}"),
    }
    assert_eq!(runtime.requests.lock().unwrap().len(), 1);

    development
        .adopt_development_candidate("mvp-run-1")
        .unwrap();
    assert_eq!(development.list_nodes().unwrap()[0].kind, "mvp_adapter");

    let validation_catalog = catalog();
    let proposal_id = development.id().to_string();
    let mut lifecycle = PluginLifecycle::new(&mut development, &validation_catalog, &[]);
    let validation = lifecycle.validate_and_submit().unwrap();
    match validation {
        ValidationOutcome::Submitted(report) => assert!(report.passed()),
        ValidationOutcome::NeedsFix(report) => panic!("validation failed: {report:?}"),
    }
    assert_eq!(lifecycle.proposal().status, ProposalStatus::PendingReview);

    proposals.approve(&proposal_id).unwrap();
    let publisher = FakePublisher::default();
    let published = lifecycle.publish_reviewed(&publisher).unwrap();
    assert_eq!(published.status, ProposalStatus::Published);
    assert_eq!(publisher.calls.lock().unwrap().as_slice(), ["mvp-plugin"]);

    let config = state.path().join("plugins.toml");
    let installed = lifecycle.install(&config).unwrap();
    assert_eq!(installed.status, ProposalStatus::Installed);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("name = \"mvp-plugin\""));
    assert!(text.contains("git = \"git@github.com:auto-nomics/mvp-plugin-plugin.git\""));
    assert!(text.contains(&format!("rev = \"{}\"", published.pushed_commit.unwrap())));
}

#[tokio::test]
async fn installed_plugin_feedback_update_completes_through_pr() {
    let state = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let remote = "git@github.com:auto-nomics/mvp-plugin-plugin.git";

    let source_workspace = plugin_rsi::ProposalWorkspace::new(source.path());
    source_workspace
        .write_text(
            "manifest.toml",
            &format!(
                r#"
schema_version = 1
plugin_name = "mvp-plugin"

[image]
reference = "{ENVIRONMENT_REFERENCE}"

[[nodes]]
kind = "mvp_adapter"
desc = "Copy one file"
doc = "Copies input 0 to output 0."

[nodes.ports]
inputs = [{{ type = "file", label = "input" }}]
outputs = [{{ path = "result.txt", format = "txt" }}]

[nodes.command]
interpreter = "sh"
script_file = "scripts/adapter.sh"
"#
            ),
        )
        .unwrap();
    source_workspace
        .write_text(
            "scripts/adapter.sh",
            "#!/bin/sh\nset -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    source_workspace
        .write_text("README.md", "# mvp-plugin\n\nA deterministic adapter.\n")
        .unwrap();
    let source_repo = GitRepo::init(source.path(), "main").unwrap();
    let base_commit = source_repo
        .snapshot_commit("install", "Autonomics RSI", "rsi@example.com")
        .unwrap()
        .unwrap();

    let config = state.path().join("plugins.toml");
    std::fs::write(
        &config,
        format!("[[plugin]]\nname = \"mvp-plugin\"\ngit = \"{remote}\"\nrev = \"{base_commit}\"\n"),
    )
    .unwrap();
    let installed = plugin_rsi::read_git_plugin_source(&config, "mvp-plugin").unwrap();
    assert_eq!(installed.commit, base_commit);

    let requests = RequestStore::open(state.path());
    let request = requests
        .record(RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::Eval,
            intent: RequestIntent::OptimizeNode,
            summary: "Optimize the installed adapter".into(),
            body: "Reject an empty input before copying it.".into(),
            plugin_name: Some("mvp-plugin".into()),
            evidence_ids: vec!["empty-input-report".into()],
            status: RequestStatus::Open,
        })
        .unwrap();
    let proposals = ProposalStore::open(state.path(), "main", "Autonomics RSI", "rsi@example.com");
    let mut development = proposals
        .create_update(
            "mvp-plugin",
            &[request.id],
            "The installed adapter silently copies an empty input.",
            &requests,
            &installed,
            &catalog(),
            &LocalSourceFetcher {
                local_path: source.path().to_path_buf(),
            },
        )
        .unwrap();
    assert_eq!(
        development.proposal().base_commit.as_deref(),
        Some(base_commit.as_str())
    );
    assert_eq!(development.list_nodes().unwrap()[0].kind, "mvp_adapter");

    let profile = AgentProfile::new("mvp-plugin-update-agent").unwrap();
    profile
        .bind_plugin(&mut development, "mvp-update-1")
        .unwrap();
    let tools = profile.tool_registrations();
    execute(
        &tools,
        "plugin_node_write_script",
        json!({
            "node_kind": "mvp_adapter",
            "contents": "#!/bin/sh\nset -eu\ntest -s \"$AUTONOMICS_INPUT0\"\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n"
        }),
    )
    .await
    .unwrap();
    development
        .adopt_development_candidate("mvp-update-1")
        .unwrap();
    development.snapshot("update: reject empty input").unwrap();

    let validation_catalog = catalog();
    let installed_kinds = ["mvp_adapter".to_string()];
    let mut lifecycle =
        PluginLifecycle::new(&mut development, &validation_catalog, &installed_kinds);
    match lifecycle.validate_and_submit().unwrap() {
        ValidationOutcome::Submitted(report) => assert!(report.passed()),
        ValidationOutcome::NeedsFix(report) => panic!("validation failed: {report:?}"),
    }
    let proposal_id = lifecycle.proposal().proposal_id.clone();
    proposals.approve(&proposal_id).unwrap();

    let publisher = FakePullRequestPublisher::default();
    let opened = lifecycle.open_update_pull_request(&publisher).unwrap();
    assert_eq!(opened.status, ProposalStatus::PullRequestOpen);
    assert_eq!(opened.pull_request_number, Some(7));
    let merged = lifecycle.merge_update_pull_request(&publisher).unwrap();
    assert_eq!(merged.status, ProposalStatus::PullRequestMerged);
    let installed_update = lifecycle.install(&config).unwrap();
    assert_eq!(installed_update.status, ProposalStatus::Installed);

    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains(&base_commit));
    assert!(text.contains(&format!("rev = \"{}\"", merged.merged_commit.unwrap())));
}
