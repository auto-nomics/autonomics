use std::sync::Arc;

use arrow_array::{Float64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use dag_core::NodeFactory;
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;
use nodes_io::pathway_gsea_container::{
    PATHWAY_GSEA_CONTAINER_KIND, PATHWAY_GSEA_IMAGE_DIGEST, PathwayGseaContainerNodeFactory,
};
use vfs::OpendalFileStorage;

fn skip_unless_enabled() -> bool {
    if std::env::var_os("AUTONOMICS_PATHWAY_GSEA_IT").is_none() {
        return true;
    }
    if PATHWAY_GSEA_IMAGE_DIGEST.starts_with("sha256:0000") {
        eprintln!("skipping: pathway-gsea image digest has not been published yet");
        return true;
    }

    let program = std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into());
    !matches!(
        std::process::Command::new(&program).arg("--version").status(),
        Ok(status) if status.success()
    )
}

#[tokio::test]
async fn real_podman_runs_fgsea_contract_when_enabled() {
    if skip_unless_enabled() {
        eprintln!("skipping: set AUTONOMICS_PATHWAY_GSEA_IT=1 with a published image to run");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let (session, storage) = OpendalFileStorage::new_temp().register_to_ctx();
    let ctx = NodeCtx::new(session.runtime_env().clone(), Some(storage));

    let genes: Vec<String> = (0..20).map(|index| format!("GENE{index:02}")).collect();
    let scores: Vec<f64> = (0..20).map(|index| 20.0 - index as f64).collect();
    let rank = arrow_array::RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("gene", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(genes.clone())),
            Arc::new(Float64Array::from(scores)),
        ],
    )
    .unwrap();
    let pathways = arrow_array::RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("pathway_name", DataType::Utf8, false),
            Field::new("gene", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                (0..20)
                    .map(|index| {
                        if index < 10 {
                            "TEST_UP".to_string()
                        } else {
                            "TEST_DOWN".to_string()
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(genes)),
        ],
    )
    .unwrap();
    let inputs = vec![
        NodeInput::new_dataframe(0, ctx.session().read_batch(rank).unwrap()),
        NodeInput::new_dataframe(1, ctx.session().read_batch(pathways).unwrap()),
    ];

    let config = container_runtime::PodmanConfig {
        workspace_root: root.path().join("workspace"),
        panel_cache_root: root.path().join("panels"),
        ..container_runtime::PodmanConfig::default()
    };
    let infra = container_runtime::ContainerExecutionInfra::from_config(config);
    let factory = PathwayGseaContainerNodeFactory::new(infra.runtime, infra.panel_cache);
    let mut node = factory
        .build(
            serde_json::json!({
                "min_size": 5,
                "max_size": 100,
                "artifact_prefix": "/artifacts/pathway-gsea-it"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), PATHWAY_GSEA_CONTAINER_KIND);

    let outputs = node
        .execute(
            &ctx,
            &inputs,
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let report = outputs.get(&0).unwrap().as_dataframe().unwrap();
    let batches = report.clone().collect().await.unwrap();
    if batches[0].num_rows() != 2 {
        let artifact_path = outputs.get(&1).unwrap().as_file().unwrap().path.clone();
        let virtual_path = artifact_path.strip_prefix("vfs://").unwrap();
        let json = ctx
            .opendal
            .as_ref()
            .unwrap()
            .resolve(virtual_path)
            .read(&ctx.opendal.as_ref().unwrap().resolve_path(virtual_path))
            .await
            .unwrap();
        eprintln!(
            "fgsea artifact: {}",
            String::from_utf8_lossy(&json.to_vec())
        );
    }
    assert_eq!(batches[0].num_rows(), 2);
    assert!(outputs.get(&1).unwrap().as_file().is_ok());
}
