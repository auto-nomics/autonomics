use plugin_rsi::{
    ApprovedImage, GateStatus, ImageCatalog, ProposalStore, RequestIntent, RequestRecord,
    RequestSource, RequestStatus, RequestStore, validate_workspace,
};

const IMAGE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c";

fn catalog() -> ImageCatalog {
    let mut catalog = ImageCatalog::default();
    catalog.insert(
        "demo",
        ApprovedImage {
            reference: IMAGE.into(),
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
        body: "Copy one file through the demo image.".into(),
        plugin_name: Some("demo-plugin".into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn write_plugin(workspace: &plugin_rsi::ProposalWorkspace) {
    workspace
        .write_text(
            "manifest.toml",
            &format!(
                r#"
schema_version = 1
plugin_name = "demo-plugin"

[image]
reference = "{IMAGE}"

[[nodes]]
kind = "demo_plugin"
desc = "Demo file adapter"
doc = "Copies input 0 to output 0."

[nodes.ports]
inputs = [{{ type = "file", label = "input" }}]
outputs = [{{ path = "out.txt", format = "txt" }}]

[nodes.command]
interpreter = "sh"
script_file = "scripts/adapter.sh"
"#
            ),
        )
        .unwrap();
    workspace
        .write_text(
            "scripts/adapter.sh",
            "set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
        )
        .unwrap();
    workspace.write_text("README.md", "# demo\n").unwrap();
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
    assert_eq!(development.proposal().image_reference, None);
    development.bind_approved_image("demo", &catalog()).unwrap();

    write_plugin(&development.workspace());
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
