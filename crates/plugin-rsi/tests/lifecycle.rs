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
    let proposal = proposals
        .create(
            "demo-plugin",
            &[request.id.clone()],
            "The request needs a deterministic adapter.",
            &requests,
        )
        .unwrap();
    assert_eq!(proposal.image_reference, None);
    proposals
        .bind_approved_image(&proposal.proposal_id, "demo", &catalog())
        .unwrap();

    write_plugin(&proposals.workspace(&proposal.proposal_id).unwrap().unwrap());
    proposals.start_validation(&proposal.proposal_id).unwrap();
    let current = proposals.find(&proposal.proposal_id).unwrap().unwrap();
    let report = validate_workspace(
        &current,
        &proposals.workspace(&proposal.proposal_id).unwrap().unwrap(),
        &catalog(),
        &[],
        1,
    );
    assert_eq!(report.overall, GateStatus::Pass, "{report:?}");
    let reports_dir = proposals.reports_dir(&proposal.proposal_id);
    let report_path = report.write(&reports_dir).unwrap();
    let relative_report = report_path
        .strip_prefix(reports_dir.parent().unwrap())
        .unwrap()
        .to_string_lossy()
        .to_string();
    proposals
        .record_report(&proposal.proposal_id, &relative_report, &report)
        .unwrap();
    proposals
        .snapshot(&proposal.proposal_id, "snapshot: attempt 1")
        .unwrap();
    proposals.submit(&proposal.proposal_id).unwrap();
    let reviewed = proposals.approve(&proposal.proposal_id).unwrap();
    assert!(reviewed.source_commit.is_some());
}
