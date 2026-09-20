use std::path::Path;
use std::sync::Arc;

use container_runtime::{PanelCache, PodmanConfig, PodmanRuntime};
use dag_core::NodeFactory;
use dag_core::dag::DAG;
use dag_core::dag::runtime::SchedulerConfig;
use dag_core::registry::NodeCtx;
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::limma_voom_container::LimmaVoomContainerNodeFactory;
use nodes_io::wgcna_container::WgcnaContainerNodeFactory;
use vfs::OpendalFileStorage;

type TestHarness = (
    tempfile::TempDir,
    NodeCtx,
    Arc<container_runtime::PodmanRuntime>,
    Arc<PanelCache>,
);

fn harness() -> TestHarness {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let workspace = root.path().join("workspace");
    let panels = root.path().join("panels");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&panels).unwrap();
    let runtime = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root: workspace,
        panel_cache_root: panels.clone(),
    }));
    (root, ctx, runtime, Arc::new(PanelCache::new(panels)))
}

async fn read_output(ctx: &NodeCtx, file: &dag_core::value::FileRef) -> Vec<u8> {
    let storage = ctx.opendal.as_ref().expect("runtime VFS");
    let path = file.path.strip_prefix("vfs://").unwrap();
    storage
        .resolve(path)
        .read(&storage.resolve_path(path))
        .await
        .unwrap()
        .to_vec()
}

#[ignore = "requires rootless Podman and the published bulk-rnaseq image"]
#[tokio::test]
async fn real_podman_runs_limma_voom_categorical_and_continuous_contracts() {
    let (root, ctx, runtime, panels) = harness();
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/deseq2/fixtures");
    let counts = fixture_root.join("pasilla_gene_counts.tsv");
    let metadata = fixture_root.join("pasilla_sample_metadata.tsv");
    let continuous_metadata = root.path().join("duration_metadata.tsv");
    let source = std::fs::read_to_string(&metadata).unwrap();
    let durations = ["1", "3", "4", "8", "11", "14", "17"];
    let mut rows = source.lines();
    let mut rewritten = String::new();
    rewritten.push_str(rows.next().unwrap());
    rewritten.push_str("\tduration\n");
    for (index, row) in rows.enumerate() {
        rewritten.push_str(row);
        rewritten.push('\t');
        rewritten.push_str(durations[index]);
        rewritten.push('\n');
    }
    std::fs::write(&continuous_metadata, rewritten).unwrap();

    let factory = LimmaVoomContainerNodeFactory::new(runtime, panels);
    for (node_name, metadata_path, spec) in [
        (
            "limma_categorical",
            metadata,
            serde_json::json!({
                "outcome_var": "condition",
                "contrast_var": "condition",
                "contrast_levels": ["untreated", "treated"],
                "covariates": ["type"],
                "normalization": "tmm",
                "artifact_prefix": "/artifacts/limma-categorical-it"
            }),
        ),
        (
            "limma_continuous",
            continuous_metadata,
            serde_json::json!({
                "outcome_var": "duration",
                "contrast_var": "duration",
                "outcome_mode": "continuous",
                "normalization": "quantile",
                "artifact_prefix": "/artifacts/limma-continuous-it"
            }),
        ),
    ] {
        let node = factory.build(spec, ctx.clone()).unwrap();
        let mut dag = DAG::default();
        dag.add_node(
            "counts".into(),
            Box::new(FileReferenceNode::new(
                counts.to_string_lossy().into_owned(),
                Some("counts".into()),
            )),
        )
        .unwrap();
        dag.add_node(
            "metadata".into(),
            Box::new(FileReferenceNode::new(
                metadata_path.to_string_lossy().into_owned(),
                Some("metadata".into()),
            )),
        )
        .unwrap();
        dag.add_node(node_name.into(), node).unwrap();
        dag.add_edge("counts", node_name, 0, 0).unwrap();
        dag.add_edge("metadata", node_name, 0, 1).unwrap();
        let report = dag
            .run(&SchedulerConfig::default(), &ctx, None)
            .await
            .unwrap();
        assert_eq!(
            report.statuses.get(node_name),
            Some(&dag_core::dag::RuntimeStatus::Success),
            "{node_name} failed: {report:#?}"
        );

        let outputs = dag.output(node_name).unwrap();
        let results = outputs.get(&0).unwrap().as_file().unwrap().clone();
        let results = read_output(&ctx, &results).await;
        assert!(results.starts_with(b"gene_id\tlogFC\t"));
        let report = outputs.get(&4).unwrap().as_file().unwrap().clone();
        let report = read_output(&ctx, &report).await;
        let report: serde_json::Value = serde_json::from_slice(&report).unwrap();
        assert_eq!(report["engine"]["versions"]["limma"], "3.66.0");
        assert_eq!(report["engine"]["versions"]["edgeR"], "4.8.2");
    }
}

#[ignore = "requires rootless Podman and the published bulk-rnaseq image"]
#[tokio::test]
async fn real_podman_runs_wgcna_pipeline_contract() {
    let (root, ctx, runtime, panels) = harness();
    let expression_path = root.path().join("expression.tsv");
    let mut expression = String::from("gene_id");
    for sample in 0..12 {
        expression.push_str(&format!("\ts{sample:02}"));
    }
    expression.push('\n');
    for gene in 0..60 {
        expression.push_str(&format!("GENE{gene:03}"));
        for sample in 0..12 {
            let latent = ((sample + 1) as f64 * std::f64::consts::PI / 6.0).sin();
            let base = if gene < 20 {
                10.0 + 3.0 * latent
            } else if gene < 40 {
                8.0 - 3.0 * latent
            } else {
                6.0
            };
            let noise = ((gene * 7 + sample * 11) % 9) as f64 / 20.0;
            expression.push_str(&format!("\t{:.6}", base + noise));
        }
        expression.push('\n');
    }
    std::fs::write(&expression_path, expression).unwrap();
    let metadata_path = root.path().join("metadata.tsv");
    let mut metadata = String::from("sample_id\tduration\n");
    for sample in 0..12 {
        metadata.push_str(&format!("s{sample:02}\t{}\n", sample + 1));
    }
    std::fs::write(&metadata_path, metadata).unwrap();

    let factory = WgcnaContainerNodeFactory::new(runtime, panels);
    let node = factory
        .build(
            serde_json::json!({
                "network_type": "signed",
                "cor": "pearson",
                "min_module_size": 5,
                "deep_split": 2,
                "merge_threshold": 0.25,
                "max_block_size": 500,
                "artifact_prefix": "/artifacts/wgcna-it"
            }),
            ctx.clone(),
        )
        .unwrap();
    let mut dag = DAG::default();
    dag.add_node(
        "expression".into(),
        Box::new(FileReferenceNode::new(
            expression_path.to_string_lossy().into_owned(),
            Some("expression".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "metadata".into(),
        Box::new(FileReferenceNode::new(
            metadata_path.to_string_lossy().into_owned(),
            Some("metadata".into()),
        )),
    )
    .unwrap();
    dag.add_node("wgcna".into(), node).unwrap();
    dag.add_edge("expression", "wgcna", 0, 0).unwrap();
    dag.add_edge("metadata", "wgcna", 0, 1).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("wgcna"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "WGCNA failed: {report:#?}"
    );

    let outputs = dag.output("wgcna").unwrap();
    let modules = outputs.get(&3).unwrap().as_file().unwrap().clone();
    let modules = read_output(&ctx, &modules).await;
    assert!(modules.starts_with(b"gene_id\tmodule_label\tmodule_color\t"));
    assert_eq!(String::from_utf8_lossy(&modules).lines().count(), 61);
    let eigengenes = outputs.get(&4).unwrap().as_file().unwrap().clone();
    let eigengenes = read_output(&ctx, &eigengenes).await;
    let eigengenes_text = String::from_utf8_lossy(&eigengenes);
    assert!(eigengenes_text.starts_with("sample_id\tME"));
    assert!(eigengenes_text.contains("\tduration\n"));
    let run_report = outputs.get(&5).unwrap().as_file().unwrap().clone();
    let run_report = read_output(&ctx, &run_report).await;
    let run_report: serde_json::Value = serde_json::from_slice(&run_report).unwrap();
    assert_eq!(run_report["engine"]["versions"]["WGCNA"], "1.74");
    assert_eq!(run_report["dimensions"]["modules"], 3);
}
