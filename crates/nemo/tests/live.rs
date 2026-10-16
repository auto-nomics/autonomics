use arrow_array::{Array, StringArray, UInt64Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use nemo::nodes::{NemoCollectionsNodeFactory, NemoDatasetsNodeFactory, NemoMatricesNodeFactory};

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
#[ignore = "live NeMO Analytics API test"]
async fn nemo_query_nodes_emit_dataframes() -> TestResult {
    let collections = run(&NemoCollectionsNodeFactory, serde_json::json!({})).await?;
    assert!(collections[0].num_rows() > 20);
    assert_eq!(collections[0].num_columns(), 6);

    let datasets = run(
        &NemoDatasetsNodeFactory,
        serde_json::json!({"limit": 5, "dataset_type": "single-cell-rnaseq"}),
    )
    .await?;
    assert_eq!(datasets[0].num_rows(), 5);
    assert_eq!(datasets[0].num_columns(), 16);
    let total = datasets[0]
        .column(15)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("dataset total");
    assert!(total.value(0) > 100);

    let matrices = run(
        &NemoMatricesNodeFactory,
        serde_json::json!({"dataset_id": "4fbd43e2-f301-42d8-a61d-a5dd5bf720e7"}),
    )
    .await?;
    assert_eq!(matrices[0].num_rows(), 1);
    assert_eq!(matrices[0].num_columns(), 13);
    let genes = matrices[0]
        .column(5)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("gene count");
    let cells = matrices[0]
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("cell count");
    assert_eq!(genes.value(0), 31_053);
    assert_eq!(cells.value(0), 10_000);
    let share_id = matrices[0]
        .column(2)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("share ID");
    assert_eq!(share_id.value(0), "00bfd6d7");

    Ok(())
}
