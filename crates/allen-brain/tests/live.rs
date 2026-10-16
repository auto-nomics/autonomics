use arrow_array::{Array, StringArray, UInt64Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use allen_brain::nodes::{
    AllenBrainAreasNodeFactory, AllenBrainGenesNodeFactory, AllenBrainTypesNodeFactory,
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
#[ignore = "live Allen Brain API test"]
async fn allen_brain_query_nodes_emit_dataframes() -> TestResult {
    let areas = run(
        &AllenBrainAreasNodeFactory,
        serde_json::json!({"graph_id": 1, "start_row": 5, "rows": 3}),
    )
    .await?;
    assert_eq!(areas[0].num_rows(), 3);
    assert_eq!(areas[0].num_columns(), 9);
    let total = areas[0]
        .column(8)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("area total");
    assert!(total.value(0) > 1000);

    let genes = run(
        &AllenBrainGenesNodeFactory,
        serde_json::json!({"symbol": "Pvalb"}),
    )
    .await?;
    assert_eq!(genes[0].num_rows(), 3);
    assert_eq!(genes[0].num_columns(), 8);

    let taxonomies = run(&AllenBrainTypesNodeFactory, serde_json::json!({})).await?;
    assert!(taxonomies[0].num_rows() >= 9);
    assert_eq!(taxonomies[0].num_columns(), 6);
    let names = taxonomies[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("taxonomy filenames");
    assert!(
        (0..names.len()).any(|index| names.value(index).starts_with("Mouse_VISp_ALM")),
        "mouse V1/ALM taxonomy missing"
    );

    let mouse = run(
        &AllenBrainTypesNodeFactory,
        serde_json::json!({"species": "mouse"}),
    )
    .await?;
    assert_eq!(mouse[0].num_rows(), 4);

    Ok(())
}
