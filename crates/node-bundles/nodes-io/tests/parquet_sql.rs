use std::sync::Arc;

use arrow_array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use dag_core::node::DagNode;
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;
use dag_core::value::FileRef;
use datafusion::prelude::SessionContext;
use nodes_io::parquet_sql::{DATAFUSION_SQL_KIND, DataFusionSqlNode};

#[tokio::test]
async fn executes_sql_over_a_parquet_file() {
    let root = tempfile::tempdir().unwrap();
    let input_path = root.path().join("cells.parquet");
    let output_path = root.path().join("summary.parquet");
    let ctx = SessionContext::new();
    let schema = Arc::new(Schema::new(vec![
        Field::new("cluster", DataType::Utf8, false),
        Field::new("n_genes", DataType::Int32, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec!["0", "0", "1"])),
            Arc::new(Int32Array::from(vec![10, 20, 30])),
        ],
    )
    .unwrap();
    ctx.read_batch(batch)
        .unwrap()
        .write_parquet(
            input_path.to_string_lossy().as_ref(),
            datafusion::dataframe::DataFrameWriteOptions::new(),
            None::<datafusion::config::TableParquetOptions>,
        )
        .await
        .unwrap();

    let node_ctx = NodeCtx::new(ctx.runtime_env(), None);
    let mut node = DataFusionSqlNode::new(
        "SELECT cluster, COUNT(*) AS n_cells FROM input GROUP BY cluster ORDER BY cluster".into(),
        "input".into(),
        output_path.to_string_lossy().into_owned(),
    );
    let input = NodeInput::file(
        0,
        FileRef::local(&input_path, Some("parquet".into())).unwrap(),
    );
    let outputs = node
        .execute(
            &node_ctx,
            &[input],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    assert_eq!(node.kind(), DATAFUSION_SQL_KIND);
    assert!(
        outputs
            .get(&0)
            .unwrap()
            .as_file()
            .unwrap()
            .path
            .ends_with("summary.parquet")
    );

    let verify = SessionContext::new();
    let summary = verify
        .read_parquet(
            output_path.to_string_lossy().as_ref(),
            datafusion::prelude::ParquetReadOptions::default(),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(summary[0].num_rows(), 2);
}
