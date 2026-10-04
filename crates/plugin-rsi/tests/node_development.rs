use std::collections::BTreeMap;

use container_plugin::node_definition::{
    CommandSpec, NodeDefinition, OutputSpec, ParamSpec, ParamType, PortKind, PortLayout, PortSpec,
};
use plugin_rsi::{
    Environment, EnvironmentCatalog, ProposalStore, RequestIntent, RequestRecord, RequestSource,
    RequestStatus, RequestStore, validate_workspace,
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

fn request() -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewNode,
        summary: "Develop node interface".into(),
        body: "Exercise parameter and port CRUD.".into(),
        plugin_name: Some("node-development-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn node_definition() -> NodeDefinition {
    NodeDefinition {
        kind: "node_development_plugin".into(),
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
    }
}

fn setup(state: &tempfile::TempDir) -> plugin_rsi::PluginDevelopment<'static> {
    let requests = RequestStore::open(state.path());
    let request = requests.record(request()).unwrap();
    let proposals = Box::leak(Box::new(ProposalStore::open(
        state.path(),
        "main",
        "Autonomics RSI",
        "rsi@example.com",
    )));
    let mut development = proposals
        .create(
            "node-development-plugin",
            &[request.id],
            "Develop the node interface through CRUD.",
            &requests,
        )
        .unwrap();
    development.bind_environment("demo", &catalog()).unwrap();
    let mut node = development.create_node(node_definition()).unwrap();
    node.write_script("set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n")
        .unwrap();
    development
        .workspace()
        .write_text("README.md", "# demo\n")
        .unwrap();
    development
}

#[test]
fn node_development_manages_params_and_ports() {
    let state = tempfile::tempdir().unwrap();
    let mut development = setup(&state);
    let mut node = development
        .node("node_development_plugin")
        .unwrap()
        .unwrap();

    let param = ParamSpec {
        r#type: ParamType::String,
        default: Some("demo".into()),
        optional: true,
        doc: Some("Demo parameter".into()),
        min: None,
        max: None,
        exclusive_min: None,
        exclusive_max: None,
        min_len: None,
        max_len: None,
        requires: Vec::new(),
    };
    node.create_param("mode", param.clone()).unwrap();
    let persisted = node.read_param("mode").unwrap().unwrap();
    assert_eq!(persisted.r#type, ParamType::String);
    assert_eq!(persisted.default, param.default);
    assert_eq!(persisted.doc, param.doc);
    assert!(node.create_param("mode", param.clone()).is_err());

    let mut updated = param;
    updated.default = Some("fast".into());
    node.update_param("mode", updated.clone()).unwrap();
    assert_eq!(
        node.read_param("mode").unwrap().unwrap().default,
        updated.default
    );
    node.delete_param("mode").unwrap();
    assert!(node.read_param("mode").unwrap().is_none());
    assert!(node.update_param("mode", updated).is_err());
    assert!(node.delete_param("mode").is_err());

    let second_input = PortSpec {
        r#type: PortKind::File,
        label: Some("reference".into()),
        accepted_formats: vec!["txt".into()],
    };
    node.create_input_port(second_input.clone()).unwrap();
    assert_eq!(node.list_input_ports().unwrap().len(), 2);
    let persisted = node.read_input_port(1).unwrap();
    assert_eq!(persisted.label, second_input.label);
    assert_eq!(persisted.accepted_formats, second_input.accepted_formats);

    let mut replacement = second_input;
    replacement.label = Some("optional reference".into());
    node.update_input_port(1, replacement).unwrap();
    node.delete_input_port(1).unwrap();
    assert_eq!(node.list_input_ports().unwrap().len(), 1);
    assert!(node.read_input_port(1).is_err());
    assert!(
        node.update_input_port(1, node.read_input_port(0).unwrap())
            .is_err()
    );
    assert!(node.delete_input_port(1).is_err());

    let second_output = OutputSpec {
        path: "metrics.txt".into(),
        format: Some("json".into()),
        label: None,
    };
    node.create_output_port(second_output.clone()).unwrap();
    assert_eq!(node.list_output_ports().unwrap().len(), 2);
    let persisted = node.read_output_port("metrics.txt").unwrap();
    assert_eq!(persisted.format, second_output.format);
    assert!(node.create_output_port(second_output.clone()).is_err());

    let mut renamed = second_output;
    renamed.path = "summary.json".into();
    node.update_output_port("metrics.txt", renamed.clone())
        .unwrap();
    assert!(node.read_output_port("metrics.txt").is_err());
    assert!(node.read_output_port("summary.json").is_ok());
    assert!(node.update_output_port("missing.txt", renamed).is_err());

    node.delete_output_port("result.txt").unwrap();
    assert!(node.read_output_port("result.txt").is_err());
    assert!(node.delete_output_port("missing.txt").is_err());
    let last_path = "summary.json";
    assert!(node.delete_output_port(last_path).is_err());

    let proposal = development.proposal().clone();
    let report = validate_workspace(&proposal, &development.workspace(), &catalog(), &[], 1);
    assert_eq!(report.overall, plugin_rsi::GateStatus::Pass, "{report:?}");
}
