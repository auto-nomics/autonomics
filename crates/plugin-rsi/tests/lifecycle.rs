use std::collections::BTreeMap;

use container_plugin::node_definition::{
    CommandSpec, NodeDefinition, OutputSpec, PortKind, PortLayout, PortSpec,
};
use plugin_rsi::{
    Environment, EnvironmentCatalog, GateStatus, ProposalStore, RequestIntent, RequestRecord,
    RequestSource, RequestStatus, RequestStore, validate_workspace,
};

const ENVIRONMENT_REFERENCE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c";

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

fn request() -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: "Create demo plugin".into(),
        body: "Copy one file through the demo environment.".into(),
        plugin_name: Some("demo-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
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

#[test]
fn greenfield_proposal_reaches_review_gate() {
    let state = tempfile::tempdir().unwrap();
    let requests = RequestStore::open(state.path());
    let request = requests.record(request()).unwrap();
    let proposals = ProposalStore::open(state.path(), "main", "Autonomics RSI", "rsi@example.com");
    let mut development = proposals
        .create(
            "demo-plugin",
            &[request.id.clone()],
            "The request needs a deterministic adapter.",
            &requests,
        )
        .unwrap();
    assert_eq!(development.proposal().environment_reference, None);
    development.bind_environment("demo", &catalog()).unwrap();

    let mut first = development
        .create_node(node_definition(
            "demo_plugin",
            "scripts/demo_plugin.sh",
            "demo_plugin.txt",
        ))
        .unwrap();
    first
        .write_script("set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n")
        .unwrap();
    first
        .update_with(|node| node.desc = "Updated demo file adapter".into())
        .unwrap();

    let mut second = development
        .create_node(node_definition(
            "demo_reverse_plugin",
            "scripts/demo_reverse_plugin.sh",
            "demo_reverse_plugin.txt",
        ))
        .unwrap();
    second
        .write_script("set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n")
        .unwrap();

    let temporary = development
        .create_node(node_definition(
            "demo_temp_plugin",
            "scripts/demo_temp_plugin.sh",
            "demo_temp_plugin.txt",
        ))
        .unwrap();
    temporary.delete().unwrap();
    assert_eq!(development.list_nodes().unwrap().len(), 2);

    development
        .workspace()
        .write_text("README.md", "# demo\n")
        .unwrap();
    development.start_validation().unwrap();
    let current = development.proposal().clone();
    let report = validate_workspace(&current, &development.workspace(), &catalog(), &[], 1);
    assert_eq!(report.overall, GateStatus::Pass, "{report:?}");
    let reports_dir = development.reports_dir();
    let report_path = report.write(&reports_dir).unwrap();
    let relative_report = report_path
        .strip_prefix(reports_dir.parent().unwrap())
        .unwrap()
        .to_string_lossy()
        .to_string();
    development
        .record_report(&relative_report, &report)
        .unwrap();
    development.snapshot("snapshot: attempt 1").unwrap();
    development.submit().unwrap();
    let reviewed = proposals.approve(development.id()).unwrap();
    assert!(reviewed.source_commit.is_some());
}
