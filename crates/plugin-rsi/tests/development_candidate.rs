use std::collections::BTreeMap;

use container_plugin::node_definition::{
    CommandSpec, NodeDefinition, OutputSpec, PortKind, PortLayout, PortSpec,
};
use plugin_rsi::{
    Environment, EnvironmentCatalog, ProposalStore, RequestIntent, RequestRecord, RequestSource,
    RequestStatus, RequestStore,
};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a515fdfaf615f80d059a09a5d78c";

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

fn setup(state: &tempfile::TempDir) -> plugin_rsi::PluginDevelopment<'static> {
    let requests = RequestStore::open(state.path());
    let request = requests
        .record(RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::User,
            intent: RequestIntent::NewNode,
            summary: "Prepare specialized development".into(),
            body: "Develop through the dedicated plugin agent.".into(),
            plugin_name: Some("candidate-plugin".into()),
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
            "candidate-plugin",
            &[request.id],
            "Use a host-owned development candidate.",
            &requests,
        )
        .unwrap();
    development.bind_environment("demo", &catalog()).unwrap();
    let mut node = development
        .create_node(NodeDefinition {
            kind: "candidate_plugin".into(),
            desc: "Demo adapter".into(),
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
                    path: "result.txt".into(),
                    format: Some("txt".into()),
                    label: None,
                }],
            },
            params: BTreeMap::new(),
            command: CommandSpec {
                interpreter: "sh".into(),
                argv: Vec::new(),
                script: None,
                script_file: Some("scripts/adapter.sh".into()),
                env: BTreeMap::new(),
                files: BTreeMap::new(),
            },
            resources: Default::default(),
        })
        .unwrap();
    node.write_script("#!/bin/sh\ncp \"$INPUT_0\" \"$OUTPUT_0\"\n")
        .unwrap();
    development
        .workspace()
        .write_text("README.md", "# candidate\n")
        .unwrap();
    development
}

#[test]
fn specialized_candidates_are_isolated_and_safely_adopted() {
    let state = tempfile::tempdir().unwrap();
    let mut development = setup(&state);
    let candidate = development
        .prepare_development_candidate("attempt-1")
        .unwrap();
    candidate
        .write_text("README.md", "# candidate updated\n")
        .unwrap();

    let changes = development
        .adopt_development_candidate("attempt-1")
        .unwrap();
    assert_eq!(changes.changed, vec!["README.md".to_string()]);
    assert_eq!(
        development.workspace().read_text("README.md").unwrap(),
        "# candidate updated\n"
    );
    assert!(
        development
            .adopt_development_candidate("attempt-1")
            .is_err()
    );
    assert!(
        development
            .prepare_development_candidate("../escape")
            .is_err()
    );
}

#[test]
fn unsafe_candidates_do_not_reach_the_proposal_repository() {
    let state = tempfile::tempdir().unwrap();
    let mut development = setup(&state);
    let candidate = development.prepare_development_candidate("unsafe").unwrap();
    candidate.write_text("Dockerfile", "FROM alpine\n").unwrap();

    assert!(development.adopt_development_candidate("unsafe").is_err());
    assert!(development.workspace().read_text("Dockerfile").is_err());
}
