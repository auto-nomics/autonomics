//! Run-report contents: per-port file assignments, input bindings, dispatch
//! ordering, and node-reported execution details.

use super::common::*;
use crate::dag::graph::DAG;
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};

#[tokio::test]
async fn run_report_row_counts_keep_dataframe_node_identity() {
    use crate::value::{FileRef, NodeValue};
    use datafusion::prelude::SessionContext;

    let session = SessionContext::new();
    let mut dag = DAG::default();
    let mut file_outputs = crate::dag::graph::PortOutputs::new();
    file_outputs.insert(0, NodeValue::File(FileRef::new("synthetic.parquet", None)));
    dag.outputs.insert("file".into(), file_outputs);
    for (identifier, size) in [("small", 2), ("large", 5), ("empty", 0)] {
        let dataframe = session
            .sql(&format!("SELECT * FROM range(0, {size})"))
            .await
            .unwrap();
        let mut outputs = crate::dag::graph::PortOutputs::new();
        outputs.insert(0, NodeValue::DataFrame(dataframe));
        dag.outputs.insert(identifier.into(), outputs);
    }
    let mut mixed = crate::dag::graph::PortOutputs::new();
    mixed.insert(0, NodeValue::File(FileRef::new("another.parquet", None)));
    mixed.insert(
        1,
        NodeValue::DataFrame(session.sql("SELECT * FROM range(0, 7)").await.unwrap()),
    );
    dag.outputs.insert("mixed".into(), mixed);
    let identities = ["file", "small", "large", "empty", "mixed", "absent"].map(str::to_owned);
    let actual = dag
        .collect_row_counts(&identities)
        .await
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let expected = std::collections::BTreeMap::from([
        ("small".to_owned(), 2),
        ("large".to_owned(), 5),
        ("empty".to_owned(), 0),
        ("mixed".to_owned(), 7),
    ]);
    let fixed = std::env::var("CACHEXIA_ROW_COUNT_FIXED").as_deref() == Ok("1");
    if let Ok(path) = std::env::var("CACHEXIA_ROW_COUNT_OBSERVATION") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "actual": actual, "expected": expected, "fixed": fixed,
                "full_DAG_engine_test": true, "live_source_or_G0": false
            }))
            .unwrap(),
        )
        .unwrap();
        if !fixed {
            assert_ne!(actual, expected);
            assert_eq!(actual["file"], 2);
            return;
        }
    }
    assert_eq!(actual, expected);
}




