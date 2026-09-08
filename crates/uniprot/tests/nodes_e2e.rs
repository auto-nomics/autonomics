//! Live integration tests for the UniProt DAG source nodes.
//!
//! These tests hit the live UniProt REST API. They require network access.
//! Run with: `cargo test -p uniprot --test nodes_e2e -- --include-ignored`

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use datafusion::prelude::SessionContext;

use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};

use uniprot::nodes::idmap::{UniprotIdmapNodeFactory, UniprotIdmapSpec};
use uniprot::nodes::search::{UniprotSearchNodeFactory, UniprotSearchSpec};
use uniprot::nodes::stream::{UniprotStreamNodeFactory, UniprotStreamSpec};

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

async fn run<F: NodeFactory>(
    factory: &F,
    spec: serde_json::Value,
) -> dag_core::dag::graph::PortOutputs {
    let ctx = node_ctx();
    let mut node = factory
        .build(spec, NodeCtx::clone(&ctx))
        .expect("build node");
    let inputs: &[NodeInput] = &[];
    let reporter = NodeReporter::noop();
    node.execute(&ctx, inputs, &reporter)
        .await
        .expect("node execute")
}

async fn batches(df: &datafusion::dataframe::DataFrame) -> Vec<RecordBatch> {
    df.clone().collect().await.expect("collect dataframe")
}

// -----------------------------------------------------------------------
// source_uniprot_search
// -----------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn search_node_emits_insulin_dataframe() {
    let spec = serde_json::json!({
        "gene": ["INS"],
        "organism_id": 9606,
        "reviewed": true,
        "size": 5,
    });
    let outputs = run(&UniprotSearchNodeFactory {}, spec).await;
    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = batches(df).await;
    let batch = &batches[0];

    let accs = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let idx = (0..accs.len())
        .find(|&i| accs.value(i) == "P01308")
        .expect("P01308 present");

    let genes = batch
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(genes.value(idx), "INS");

    let entry_names = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(entry_names.value(idx), "INS_HUMAN");
}

// -----------------------------------------------------------------------
// source_uniprot_stream
// -----------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn stream_node_writes_tsv_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spike.tsv");
    let spec = serde_json::json!({
        "query": "accession:(P01308 OR P0DTC2)",
        "fields": ["accession", "id"],
        "format": "tsv",
        "path": path.to_str().unwrap(),
    });

    let ctx = node_ctx();
    let mut node = UniprotStreamNodeFactory {}
        .build(spec, NodeCtx::clone(&ctx))
        .expect("build node");
    let reporter = NodeReporter::noop();
    let outputs = node
        .execute(&ctx, &[], &reporter)
        .await
        .expect("node execute");

    let file = outputs
        .get(&0)
        .and_then(|v| v.as_file().ok())
        .expect("file output");
    assert_eq!(file.path, path.to_str().unwrap());
    assert_eq!(file.format.as_deref(), Some("tsv"));

    let text = std::fs::read_to_string(&path).expect("file on disk");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("Entry\tEntry Name"));
    let ids: Vec<&str> = lines.map(|l| l.split('\t').nth(1).unwrap_or("")).collect();
    assert!(ids.contains(&"INS_HUMAN"));
    assert!(ids.contains(&"SPIKE_SARS2"));

    // sink_path surfaces the destination in node reports.
    assert_eq!(node.sink_path(), Some(path.to_str().unwrap()));
}

// -----------------------------------------------------------------------
// source_uniprot_idmap
// -----------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test (server-side job, polls up to ~1 min)"]
async fn idmap_node_maps_spec_ids() {
    let spec = serde_json::json!({
        "from_db": "Gene_Name",
        "to_db": "UniProtKB",
        "ids": ["INS", "GCG"],
    });
    let outputs = run(&UniprotIdmapNodeFactory {}, spec).await;
    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = batches(df).await;
    let batch = &batches[0];

    let accs = batch
        .column(2)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let found = (0..accs.len()).any(|i| !accs.is_null(i) && accs.value(i) == "P01308");
    assert!(found, "P01308 among mapped accessions");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test (server-side job, polls up to ~1 min)"]
async fn idmap_node_reads_ids_from_input_table() {
    // Input table: one string column of gene names.
    let session = SessionContext::new();
    let input_batch: RecordBatch = {
        use arrow_schema::{DataType, Field, Schema};
        let schema = Arc::new(Schema::new(vec![Field::new(
            "gene_symbol",
            DataType::Utf8,
            true,
        )]));
        let col = StringArray::from(vec![Some("INS"), None, Some("GCG")]);
        RecordBatch::try_new(schema, vec![Arc::new(col) as Arc<dyn Array>]).unwrap()
    };
    let input_df = session.read_batch(input_batch).unwrap();

    let spec = serde_json::json!({
        "from_db": "Gene_Name",
        "to_db": "UniProtKB",
        "id_column": "gene_symbol",
    });
    let ctx = node_ctx();
    let mut node = UniprotIdmapNodeFactory {}
        .build(spec, NodeCtx::clone(&ctx))
        .expect("build node");
    let inputs = vec![NodeInput::new_dataframe(0, input_df)];
    let reporter = NodeReporter::noop();
    let outputs = node
        .execute(&ctx, &inputs, &reporter)
        .await
        .expect("node execute");

    let df = outputs.dataframe(0).expect("dataframe output");
    let batches = batches(df).await;
    let batch = &batches[0];

    // Note: Gene_Name=INS expands to INS entries across every organism
    // (~1k rows), so assert expansion rather than an exact row count.
    assert!(batch.num_rows() >= 2, "at least the two input IDs map");
    let ids = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let id_values: Vec<&str> = (0..ids.len()).map(|i| ids.value(i)).collect();
    assert!(
        id_values.contains(&"INS"),
        "input id preserved, got {id_values:?}"
    );

    let accs = batch
        .column(2)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let found = (0..accs.len()).any(|i| !accs.is_null(i) && accs.value(i) == "P01308");
    assert!(found, "P01308 among mapped accessions");
}

// -----------------------------------------------------------------------
// Spec validation (no network)
// -----------------------------------------------------------------------

#[test]
fn spec_defaults_deserialize() {
    let search: UniprotSearchSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(search.size, None);

    let stream: UniprotStreamSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(stream.query, "");

    let idmap: UniprotIdmapSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(idmap.from_db, "");
}
