use arrow_array::{Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use tisch::nodes::{TischAssetsNodeFactory, TischCellTypesNodeFactory, TischDatasetsNodeFactory};

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
#[ignore = "live TISCH2 page test"]
async fn tisch_query_nodes_emit_dataframes() -> TestResult {
    let datasets = run(
        &TischDatasetsNodeFactory,
        serde_json::json!({"dataset": "AEL_GSE142213"}),
    )
    .await?;
    assert_eq!(datasets[0].num_rows(), 1);
    assert_eq!(datasets[0].num_columns(), 9);
    let names = datasets[0]
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("dataset names");
    assert_eq!(names.value(0), "AEL_GSE142213");

    let all_datasets = run(&TischDatasetsNodeFactory, serde_json::json!({})).await?;
    assert_eq!(all_datasets[0].num_rows(), 190);

    let cell_types = run(&TischCellTypesNodeFactory, serde_json::json!({})).await?;
    assert!(cell_types[0].num_rows() > 50);
    assert_eq!(cell_types[0].num_columns(), 2);

    let assets = run(
        &TischAssetsNodeFactory,
        serde_json::json!({"dataset": "AEL_GSE142213"}),
    )
    .await?;
    assert_eq!(assets[0].num_rows(), 2);
    assert_eq!(assets[0].num_columns(), 3);

    Ok(())
}
