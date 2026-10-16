use arrow_array::{Array, StringArray, UInt32Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use scea::nodes::{SceaExperimentNodeFactory, SceaExperimentsNodeFactory, SceaMarkersNodeFactory};

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
#[ignore = "live Single Cell Expression Atlas API test"]
async fn scea_query_nodes_emit_dataframes() -> TestResult {
    let experiments = run(
        &SceaExperimentsNodeFactory,
        serde_json::json!({
            "query": "metastatic renal cell carcinoma",
            "species": "Homo sapiens",
            "max_results": 5
        }),
    )
    .await?;
    assert_eq!(experiments[0].num_rows(), 1);
    let accessions = experiments[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("accessions");
    assert_eq!(accessions.value(0), "E-CURD-10");

    let experiment = run(
        &SceaExperimentNodeFactory,
        serde_json::json!({"accession": "E-CURD-10"}),
    )
    .await?;
    assert_eq!(experiment[0].num_rows(), 1);
    assert_eq!(experiment[0].num_columns(), 11);

    let markers = run(
        &SceaMarkersNodeFactory,
        serde_json::json!({"accession": "E-CURD-10", "k": 3}),
    )
    .await?;
    assert_eq!(markers[0].num_rows(), 45);
    assert_eq!(markers[0].num_columns(), 10);
    let k_values = markers[0]
        .column(1)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("k values");
    assert_eq!(k_values.value(0), 3);

    Ok(())
}
