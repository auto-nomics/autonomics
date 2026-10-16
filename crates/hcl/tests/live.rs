use arrow_array::{Array, StringArray, UInt32Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use hcl::nodes::{HclFilesNodeFactory, HclReleaseNodeFactory, HclVersionsNodeFactory};

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
async fn hcl_query_nodes_emit_dataframes() -> TestResult {
    let versions = run(&HclVersionsNodeFactory, serde_json::json!({})).await?;
    assert_eq!(versions[0].num_rows(), 4);
    assert_eq!(versions[0].num_columns(), 2);

    let release = run(&HclReleaseNodeFactory, serde_json::json!({"version": 4})).await?;
    assert_eq!(release[0].num_rows(), 1);
    assert_eq!(release[0].num_columns(), 16);
    let version = release[0]
        .column(1)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("release version");
    assert_eq!(version.value(0), 4);

    let files = run(&HclFilesNodeFactory, serde_json::json!({"version": 4})).await?;
    assert_eq!(files[0].num_rows(), 9);
    assert_eq!(files[0].num_columns(), 6);
    let names = files[0]
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("filenames");
    assert!(
        (0..names.len()).any(|index| names.value(index) == "HCL_Fig1_adata.h5ad"),
        "HCL H5AD missing from figshare release"
    );

    let h5ad = run(
        &HclFilesNodeFactory,
        serde_json::json!({"version": 4, "asset_type": "hcl_h5ad"}),
    )
    .await?;
    assert_eq!(h5ad[0].num_rows(), 1);

    Ok(())
}
