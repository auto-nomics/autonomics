use arrow_array::{Array, StringArray, UInt32Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use tabula_sapiens::nodes::{
    TabulaSapiensFilesNodeFactory, TabulaSapiensReleaseNodeFactory,
    TabulaSapiensVersionsNodeFactory,
};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

async fn run(
    factory: &dyn NodeFactory,
    spec: serde_json::Value,
) -> TestResult<Vec<arrow_array::RecordBatch>> {
    let ctx = node_ctx();
    let mut node = factory.build(spec, NodeCtx::clone(&ctx))?;
    let inputs: &[NodeInput] = &[];
    let outputs = node.execute(&ctx, inputs, &NodeReporter::noop()).await?;
    let dataframe = outputs.dataframe(0).map_err(std::io::Error::other)?;
    Ok(dataframe.clone().collect().await?)
}

#[tokio::test]
#[ignore = "live figshare API test"]
async fn tabula_sapiens_query_nodes_emit_dataframes() -> TestResult {
    let versions = run(&TabulaSapiensVersionsNodeFactory, serde_json::json!({})).await?;
    assert_eq!(versions[0].num_rows(), 5);
    assert_eq!(versions[0].num_columns(), 2);

    let release = run(
        &TabulaSapiensReleaseNodeFactory,
        serde_json::json!({"version": 5}),
    )
    .await?;
    assert_eq!(release[0].num_rows(), 1);
    assert_eq!(release[0].num_columns(), 12);
    let version = release[0]
        .column(1)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("release version");
    assert_eq!(version.value(0), 5);

    let files = run(
        &TabulaSapiensFilesNodeFactory,
        serde_json::json!({"version": 5}),
    )
    .await?;
    assert_eq!(files[0].num_rows(), 31);
    assert_eq!(files[0].num_columns(), 6);
    let names = files[0]
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("filenames");
    assert!(
        (0..names.len()).any(|index| names.value(index) == "Tabula_Sapiens_metadata.csv"),
        "metadata CSV missing from figshare release"
    );

    let organs = run(
        &TabulaSapiensFilesNodeFactory,
        serde_json::json!({"version": 5, "asset_type": "organ_h5ad"}),
    )
    .await?;
    assert_eq!(organs[0].num_rows(), 25);

    Ok(())
}
