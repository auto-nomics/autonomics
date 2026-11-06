//! Live integration tests for `source_gwascatalog_summary_associations`
//! against the real EBI FTP (HTTPS mirror).
//!
//! These hit the network. Run with:
//! ```sh
//! cargo test -p gwascatalog-sdk --test summary_associations_e2e -- --ignored
//! ```

use arrow_array::{Array, Float64Array, StringArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;

use gwascatalog_sdk::nodes::summary_associations::SummaryAssociationsNodeFactory;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

async fn run(spec: serde_json::Value) -> TestResult<Vec<arrow_array::RecordBatch>> {
    let ctx = node_ctx();
    let mut node = SummaryAssociationsNodeFactory
        .build(spec, NodeCtx::clone(&ctx))?;
    let inputs: &[NodeInput] = &[];
    let outputs = node.execute(&ctx, inputs, &NodeReporter::noop()).await?;
    let dataframe = outputs.dataframe(0).map_err(std::io::Error::other)?;
    Ok(dataframe.clone().collect().await?)
}

/// The reference study used across the download e2e suite: small, fully
/// harmonised, with a raw buildGRCh37 file at the root.
const ACCESSION: &str = "GCST90000061";

#[tokio::test]
#[ignore]
async fn harmonised_rows_from_ftp_mirror() -> TestResult {
    let batches = run(serde_json::json!({
        "study_accession": ACCESSION,
        "max_results": 5,
    }))
    .await?;
    let batch = batches.first().expect("node emitted a RecordBatch");
    assert_eq!(batch.num_columns(), 15);
    assert!(batch.num_rows() > 0 && batch.num_rows() <= 5, "{} rows", batch.num_rows());

    // Trait label comes from the -meta.yaml sidecar.
    let trait_col = batch
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(trait_col.value(0), "Pulse pressure");
    // Every row carries the queried accession.
    let study_col = batch
        .column(3)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    for i in 0..batch.num_rows() {
        assert_eq!(study_col.value(i), ACCESSION);
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn p_upper_window_filters_rows() -> TestResult {
    let batches = run(serde_json::json!({
        "study_accession": ACCESSION,
        "p_upper": 0.5,
        "max_results": 100,
    }))
    .await?;
    let batch = batches.first().expect("node emitted a RecordBatch");
    let p = batch
        .column(14)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    for i in 0..batch.num_rows() {
        assert!(p.value(i) <= 0.5, "row {i} p={} leaked past the filter", p.value(i));
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn raw_reveal_uses_fallback_columns() -> TestResult {
    let batches = run(serde_json::json!({
        "study_accession": ACCESSION,
        "reveal": "raw",
        "max_results": 5,
    }))
    .await?;
    let batch = batches.first().expect("node emitted a RecordBatch");
    assert!(batch.num_rows() > 0);
    // Raw submissions carry no hm_code.
    let codes = batch.column(5);
    for i in 0..batch.num_rows() {
        assert!(codes.is_null(i));
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn variant_id_query_is_rejected_with_guidance() -> TestResult {
    let err = run(serde_json::json!({"variant_id": "rs7329174"}))
        .await
        .expect_err("variant_id must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("variant_id"), "{msg}");
    assert!(msg.contains("study_accession"), "{msg}");
    Ok(())
}
