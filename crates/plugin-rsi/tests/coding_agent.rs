use std::collections::BTreeMap;

use coding_agent::{
    CodingAgentConfig, CodingAgentKind, CodingAgentRunner, CodingCommand, CodingProcess,
    CodingProcessOutput, CodingTask,
};
use container_plugin::node_definition::{
    CommandSpec, NodeDefinition, OutputSpec, PortKind, PortLayout, PortSpec,
};
use plugin_rsi::{
    Environment, EnvironmentCatalog, ProposalStore, RequestIntent, RequestRecord, RequestSource,
    RequestStatus, RequestStore, validate_workspace,
};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a515fdfaf615f80d059a09b5d78c";

fn catalog() -> EnvironmentCatalog {
    let mut catalog = EnvironmentCatalog::default();
    catalog.insert(
        "demo",
        Environment {
            reference: ENVIRONMENT_REFERENCE.into(),
            interpreters: vec!["sh".into()],
        },
    );
    catalog
}

fn node_definition(kind: &str, script_file: &str, output: &str) -> NodeDefinition {
    NodeDefinition {
        kind: kind.into(),
        desc: format!("Demo {kind} adapter"),
        doc: "Copies input 0 to output 0.".into(),
        deprecated: false,
        timeout_secs: 3600,
        artifact_prefix: None,
        ports: PortLayout {
            inputs: vec![PortSpec {
                r#type: PortKind::File,
                label: Some("input".into()),
                accepted_formats: Vec::new(),
            }],
            outputs: vec![OutputSpec {
                path: output.into(),
                format: Some("txt".into()),
                label: None,
            }],
        },
        params: BTreeMap::new(),
        command: CommandSpec {
            interpreter: "sh".into(),
            argv: Vec::new(),
            script: None,
            script_file: Some(script_file.into()),
            env: BTreeMap::new(),
            files: BTreeMap::new(),
        },
        resources: Default::default(),
    }
}

struct AddNodeProcess;

impl CodingProcess for AddNodeProcess {
    fn execute(&self, command: CodingCommand) -> coding_agent::Result<CodingProcessOutput> {
        let workspace = plugin_rsi::ProposalWorkspace::new(command.working_directory.clone());
        let mut manifest: container_plugin::manifest::PluginManifest =
            toml::from_str(&workspace.read_text("manifest.toml").unwrap()).unwrap();
        manifest.nodes.push(node_definition(
            "agent_added",
            "scripts/agent_added.sh",
            "agent_added.txt",
        ));
        workspace
            .write_text("manifest.toml", &toml::to_string(&manifest).unwrap())
            .unwrap();
        workspace
            .write_text(
                "scripts/agent_added.sh",
                "set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
            )
            .unwrap();
        Ok(CodingProcessOutput {
            exit_code: Some(0),
            success: true,
            stdout: b"added one node".to_vec(),
            stderr: Vec::new(),
            duration_ms: 3,
        })
    }
}

struct UnsafeProcess;

impl CodingProcess for UnsafeProcess {
    fn execute(&self, command: CodingCommand) -> coding_agent::Result<CodingProcessOutput> {
        let workspace = plugin_rsi::ProposalWorkspace::new(command.working_directory.clone());
        workspace.write_text("Dockerfile", "FROM alpine\n").unwrap();
        Ok(CodingProcessOutput {
            exit_code: Some(0),
            success: true,
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration_ms: 1,
        })
    }
}

fn setup(state: &tempfile::TempDir) -> plugin_rsi::PluginDevelopment<'static> {
    let requests = RequestStore::open(state.path());
    let request = requests
        .record(RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::User,
            intent: RequestIntent::NewNode,
            summary: "Agent adds demo node".into(),
            body: "Add a deterministic copy node.".into(),
            plugin_name: Some("agent-plugin".into()),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        })
        .unwrap();
    let proposals = Box::leak(Box::new(ProposalStore::open(
        state.path(),
        "main",
        "Autonomics RSI",
        "rsi@example.com",
    )));
    let mut development = proposals
        .create(
            "agent-plugin",
            &[request.id],
            "Develop through an external coding agent.",
            &requests,
        )
        .unwrap();
    development.bind_environment("demo", &catalog()).unwrap();
    let mut node = development
        .create_node(node_definition(
            "agent_plugin",
            "scripts/agent_plugin.sh",
            "agent_plugin.txt",
        ))
        .unwrap();
    node.write_script("set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n")
        .unwrap();
    development
        .workspace()
        .write_text("README.md", "# agent plugin\n")
        .unwrap();
    development
}

#[test]
fn coding_agent_changes_are_adopted_and_revalidated() {
    let state = tempfile::tempdir().unwrap();
    let mut development = setup(&state);
    let agent = CodingAgentRunner::with_process(
        CodingAgentConfig::new(
            CodingAgentKind::Custom("fake".into()),
            "fake-agent",
            vec!["--develop".into()],
        )
        .unwrap(),
        AddNodeProcess,
    );

    let run = development
        .run_coding_agent(
            &agent,
            CodingTask::new("Add one deterministic node named agent_added.")
                .with_request_context("Request: add a copy node."),
        )
        .unwrap();

    assert!(run.success);
    assert!(run.synced);
    assert!(
        run.added_files
            .contains(&"scripts/agent_added.sh".to_string())
    );
    assert!(run.changed_files.contains(&"manifest.toml".to_string()));
    assert_eq!(development.list_nodes().unwrap().len(), 2);
    assert_eq!(
        development.proposal().node_kinds,
        vec!["agent_plugin".to_string(), "agent_added".to_string()]
    );

    let report = validate_workspace(
        development.proposal(),
        &development.workspace(),
        &catalog(),
        &[],
        1,
    );
    assert!(report.passed(), "{report:?}");
}

#[test]
fn unsafe_candidate_is_rejected_without_entering_repository() {
    let state = tempfile::tempdir().unwrap();
    let mut development = setup(&state);
    let agent = CodingAgentRunner::with_process(
        CodingAgentConfig::new(
            CodingAgentKind::Custom("unsafe".into()),
            "unsafe-agent",
            vec![],
        )
        .unwrap(),
        UnsafeProcess,
    );

    let error = development
        .run_coding_agent(&agent, CodingTask::new("Try to add an image."))
        .unwrap_err();
    assert!(error.to_string().contains("Dockerfile"));
    assert!(
        !development
            .workspace()
            .list_files()
            .unwrap()
            .iter()
            .any(|path| path == "Dockerfile")
    );
    assert_eq!(development.list_nodes().unwrap().len(), 1);

    let coding_reports = development.reports_dir().join("coding");
    assert_eq!(std::fs::read_dir(coding_reports).unwrap().count(), 1);
}
