use arrow_array::{Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use hubmap::nodes::{
    HubmapDatasetNodeFactory, HubmapFilesNodeFactory, HubmapGlobusNodeFactory,
    HubmapSearchNodeFactory,
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
#[ignore = "live HuBMAP Search and Entity API test"]
async fn hubmap_query_nodes_emit_dataframes() -> TestResult {
    const DATASET_ID: &str = "b6f97ee66dcb52eada045146b6583171";

    let search = run(
        &HubmapSearchNodeFactory,
        serde_json::json!({
            "query": "HBM537.HTKP.258",
            "entity_type": "Dataset",
            "size": 10
        }),
    )
    .await?;
    assert!(search[0].num_rows() > 0);
    assert_eq!(search[0].num_columns(), 15);
    let uuids = search[0]
        .column(2)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("UUIDs");
    assert!(
        (0..uuids.len()).any(|index| uuids.value(index) == DATASET_ID),
        "stable HuBMAP dataset missing from search page"
    );

    let dataset = run(
        &HubmapDatasetNodeFactory,
        serde_json::json!({"identifier": "HBM537.HTKP.258"}),
    )
    .await?;
    assert_eq!(dataset[0].num_rows(), 1);
    assert_eq!(dataset[0].num_columns(), 20);

    let files = run(
        &HubmapFilesNodeFactory,
        serde_json::json!({"identifier": DATASET_ID, "file_type": "json"}),
    )
    .await?;
    assert_eq!(files[0].num_rows(), 1);
    assert_eq!(files[0].num_columns(), 8);

    let globus = run(
        &HubmapGlobusNodeFactory,
        serde_json::json!({"identifier": DATASET_ID}),
    )
    .await?;
    assert_eq!(globus[0].num_rows(), 1);
    let url = globus[0]
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("Globus URL");
    assert!(url.value(0).starts_with("https://app.globus.org/"));

    Ok(())
}
