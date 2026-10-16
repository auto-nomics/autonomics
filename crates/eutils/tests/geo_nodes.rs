//! Live integration tests for GEO query DAG source nodes.

use arrow_array::{Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use eutils::nodes::geo::{
    GeoEGQueryNodeFactory, GeoEInfoNodeFactory, GeoELinkNodeFactory, GeoESearchNodeFactory,
    GeoESpellNodeFactory, GeoESummaryNodeFactory,
};

mod common;

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

fn one_batch(batches: &[arrow_array::RecordBatch]) -> &arrow_array::RecordBatch {
    batches
        .first()
        .expect("GEO node emitted at least one RecordBatch")
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_einfo_node_emits_database_fields() -> TestResult {
    let batches = run(&GeoEInfoNodeFactory, serde_json::json!({"database": "gds"})).await?;
    let batch = one_batch(&batches);
    assert!(batch.num_rows() > 0);
    assert!(batch.num_columns() == 11);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_esearch_node_emits_uid_page() -> TestResult {
    common::rate_limit();
    let batches = run(
        &GeoESearchNodeFactory,
        serde_json::json!({"accession": "GSE48094", "retmax": 5}),
    )
    .await?;
    let batch = one_batch(&batches);
    assert_eq!(batch.num_rows(), 1);
    let uid = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("uid strings");
    assert_eq!(uid.value(0), "200048094");

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_esummary_node_emits_series_metadata() -> TestResult {
    common::rate_limit();
    let batches = run(
        &GeoESummaryNodeFactory,
        serde_json::json!({"uids": ["200048094"]}),
    )
    .await?;
    let batch = one_batch(&batches);
    assert_eq!(batch.num_rows(), 1);
    let accession = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("accession strings");
    assert_eq!(accession.value(0), "GSE48094");
    let ftp_link = batch
        .column(17)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("FTP strings");
    assert_eq!(
        ftp_link.value(0),
        "ftp://ftp.ncbi.nlm.nih.gov/geo/series/GSE48nnn/GSE48094/"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_elink_node_emits_pubmed_link() -> TestResult {
    common::rate_limit();
    let batches = run(
        &GeoELinkNodeFactory,
        serde_json::json!({"uids": ["200048094"], "database": "pubmed"}),
    )
    .await?;
    let batch = one_batch(&batches);
    assert_eq!(batch.num_rows(), 1);
    let destination = batch
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("destination UID strings");
    assert_eq!(destination.value(0), "23946636");

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_egquery_node_emits_database_counts() -> TestResult {
    if !common::egquery_available().await {
        return Ok(());
    }
    common::rate_limit();
    let batches = run(
        &GeoEGQueryNodeFactory,
        serde_json::json!({"term": "GSE48094"}),
    )
    .await?;
    let batch = one_batch(&batches);
    assert!(batch.num_rows() > 1);
    assert!(batch.column(0).len() == batch.num_rows());

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_espell_node_emits_query_metadata() -> TestResult {
    if !common::espell_available().await {
        return Ok(());
    }
    common::rate_limit();
    let batches = run(
        &GeoESpellNodeFactory,
        serde_json::json!({"database": "gds", "term": "single cell RNA seq"}),
    )
    .await?;
    let batch = one_batch(&batches);
    assert_eq!(batch.num_rows(), 1);
    let original = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("original query strings");
    assert_eq!(original.value(0), "single cell RNA seq");

    Ok(())
}
