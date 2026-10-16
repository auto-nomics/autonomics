use arrow_array::{Array, Int64Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use kpmp::nodes::{
    KpmpDataTypesNodeFactory, KpmpParticipantDatasetsNodeFactory, KpmpParticipantNodeFactory,
    KpmpParticipantRepositoryNodeFactory, KpmpRepositoryNodeFactory,
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
#[ignore = "live KPMP Atlas GraphQL test"]
async fn kpmp_query_nodes_emit_dataframes() -> TestResult {
    const REDCAP_ID: &str = "164-1";

    let data_types = run(
        &KpmpDataTypesNodeFactory,
        serde_json::json!({"query": "single-nucleus"}),
    )
    .await?;
    assert_eq!(data_types[0].num_rows(), 1);
    assert_eq!(data_types[0].num_columns(), 9);

    let repository = run(
        &KpmpRepositoryNodeFactory,
        serde_json::json!({"query": "Single-nucleus"}),
    )
    .await?;
    assert_eq!(repository[0].num_rows(), 1);
    assert_eq!(repository[0].num_columns(), 9);
    let total_files = repository[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("total files");
    assert!(total_files.value(0) > 20_000);

    let participant = run(
        &KpmpParticipantNodeFactory,
        serde_json::json!({"redcap_id": REDCAP_ID}),
    )
    .await?;
    assert_eq!(participant[0].num_rows(), 1);
    assert_eq!(participant[0].num_columns(), 20);
    let participant_id = participant[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("participant ID");
    assert_eq!(participant_id.value(0), "208");

    let datasets = run(
        &KpmpParticipantDatasetsNodeFactory,
        serde_json::json!({"redcap_id": REDCAP_ID}),
    )
    .await?;
    assert_eq!(datasets[0].num_rows(), 13);
    assert_eq!(datasets[0].num_columns(), 5);

    let participant_repository = run(
        &KpmpParticipantRepositoryNodeFactory,
        serde_json::json!({"redcap_id": REDCAP_ID, "data_type": "Imaging"}),
    )
    .await?;
    assert_eq!(participant_repository[0].num_rows(), 1);
    assert_eq!(participant_repository[0].num_columns(), 6);
    let file_count = participant_repository[0]
        .column(2)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("file count");
    assert_eq!(file_count.value(0), 21);

    Ok(())
}
