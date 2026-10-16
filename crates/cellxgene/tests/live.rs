use arrow_array::{Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use cellxgene::nodes::{
    CellxgeneAssetsNodeFactory, CellxgeneCollectionNodeFactory, CellxgeneCollectionsNodeFactory,
    CellxgeneDatasetsNodeFactory,
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
#[ignore = "live CZ CELLxGENE Discover API test"]
async fn cellxgene_query_nodes_emit_dataframes() -> TestResult {
    let collections = run(
        &CellxgeneCollectionsNodeFactory,
        serde_json::json!({"query": "retina", "max_results": 5}),
    )
    .await?;
    assert_eq!(collections.len(), 1);
    assert!(collections[0].num_rows() > 0);
    let collection_ids = collections[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("collection IDs");
    let collection_id = (0..collection_ids.len())
        .map(|index| collection_ids.value(index))
        .find(|id| *id == "af893e86-8e9f-41f1-a474-ef05359b1fb7")
        .expect("retina collection")
        .to_owned();
    assert_eq!(collection_id, "af893e86-8e9f-41f1-a474-ef05359b1fb7");

    let collection = run(
        &CellxgeneCollectionNodeFactory,
        serde_json::json!({"collection_id": collection_id}),
    )
    .await?;
    assert_eq!(collection[0].num_rows(), 1);

    let datasets = run(
        &CellxgeneDatasetsNodeFactory,
        serde_json::json!({
            "collection_id": collection_id,
            "organism": "Homo sapiens",
            "max_results": 20
        }),
    )
    .await?;
    assert_eq!(datasets[0].num_rows(), 7);
    let dataset_ids = datasets[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("dataset IDs");
    let dataset_id = (0..dataset_ids.len())
        .map(|index| dataset_ids.value(index))
        .find(|id| *id == "7f2413c4-38c2-4559-a959-e649e2118586")
        .expect("retina dataset")
        .to_owned();
    assert_eq!(dataset_id, "7f2413c4-38c2-4559-a959-e649e2118586");

    let assets = run(
        &CellxgeneAssetsNodeFactory,
        serde_json::json!({"dataset_id": dataset_id, "filetype": "H5AD"}),
    )
    .await?;
    assert_eq!(assets[0].num_rows(), 1);
    assert_eq!(assets[0].column(3).len(), 1);

    Ok(())
}
