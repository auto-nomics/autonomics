use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use datafusion::prelude::{SessionContext, cast, col, lit};

use super::DAG;
use crate::dag::graph::PortOutputs;
use crate::value::{FileRef, NodeValue};

fn file_output(dag: &mut DAG, identifier: &str) {
    let mut outputs = PortOutputs::new();
    outputs.insert(0, NodeValue::File(FileRef::new("synthetic.parquet", None)));
    dag.outputs.insert(identifier.into(), outputs);
}

async fn dataframe_output(dag: &mut DAG, identifier: &str, size: usize, port: u8) {
    let dataframe = SessionContext::new()
        .sql(&format!("SELECT * FROM range(0, {size})"))
        .await
        .unwrap();
    dag.outputs
        .entry(identifier.into())
        .or_default()
        .insert(port, NodeValue::DataFrame(dataframe));
}

async fn counts(dag: &DAG, identifiers: &[&str]) -> BTreeMap<String, usize> {
    let identifiers = identifiers
        .iter()
        .map(|identifier| (*identifier).to_owned())
        .collect::<Vec<_>>();
    dag.collect_row_counts(&identifiers)
        .await
        .into_iter()
        .collect()
}

#[tokio::test]
async fn non_dataframe_output_does_not_steal_following_counts() {
    let mut dag = DAG::default();
    file_output(&mut dag, "file");
    dataframe_output(&mut dag, "small", 2, 0).await;
    dataframe_output(&mut dag, "large", 5, 0).await;
    assert_eq!(
        counts(&dag, &["file", "small", "large"]).await,
        BTreeMap::from([("small".into(), 2), ("large".into(), 5)])
    );
}

#[tokio::test]
async fn genuine_empty_dataframe_count_remains_zero_for_its_own_identity() {
    let mut dag = DAG::default();
    file_output(&mut dag, "file");
    dataframe_output(&mut dag, "empty", 0, 0).await;
    dataframe_output(&mut dag, "nonempty", 3, 0).await;
    assert_eq!(
        counts(&dag, &["file", "empty", "nonempty"]).await,
        BTreeMap::from([("empty".into(), 0), ("nonempty".into(), 3)])
    );
}

#[tokio::test]
async fn secondary_dataframe_port_retains_its_node_identity() {
    let mut dag = DAG::default();
    file_output(&mut dag, "file");
    file_output(&mut dag, "mixed");
    dataframe_output(&mut dag, "mixed", 7, 1).await;
    dataframe_output(&mut dag, "other", 3, 0).await;
    assert_eq!(
        counts(&dag, &["file", "mixed", "other", "absent"]).await,
        BTreeMap::from([("mixed".into(), 7), ("other".into(), 3)])
    );
}

#[tokio::test]
async fn failed_count_is_unknown_not_a_fabricated_empty_table() {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "value",
        DataType::Utf8,
        false,
    )]));
    let batch =
        RecordBatch::try_new(schema, vec![Arc::new(StringArray::from(vec!["invalid"]))]).unwrap();
    let dataframe = SessionContext::new()
        .read_batch(batch)
        .unwrap()
        .filter(cast(col("value"), DataType::Int64).gt(lit(0_i64)))
        .unwrap();
    assert!(dataframe.clone().count().await.is_err());
    let mut dag = DAG::default();
    let mut outputs = PortOutputs::new();
    outputs.insert(0, NodeValue::DataFrame(dataframe));
    dag.outputs.insert("failing".into(), outputs);
    assert!(counts(&dag, &["failing"]).await.is_empty());
}
