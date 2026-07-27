//! File sink node: consumes an upstream `DataFrame` and writes it to a file
//! (CSV or Parquet).
//!
//! One untyped input port; no output ports. Symmetric to [`crate::nodes::FileSourceNode`]
//! for the file case.

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::{
    common::HashMap,
    common::config::{CsvOptions, TableParquetOptions},
    dataframe::{DataFrame, DataFrameWriteOptions},
    execution::runtime_env::RuntimeEnv,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::sink_common::SinkMode;
use super::source_file::normalize_path;
use crate::{
    dag::DagError,
    dag::graph::PortOutputs,
    node_registry::registry::{NodeCtx, NodeFactory, new_isolated_ctx},
};

/// Supported on-disk write formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WriteFormat {
    Csv,
    Parquet,
}

#[derive(Debug, Error)]
pub enum FileSinkError {
    #[error("Invalid input: {message}")]
    InvalidInput { message: String },
    #[error("write sink '{path}' failed")]
    Write {
        path: String,
        #[source]
        source: datafusion::error::DataFusionError,
    },
}

impl From<FileSinkError> for DagError {
    fn from(e: FileSinkError) -> Self {
        match e {
            FileSinkError::Write { source, .. } => DagError::DataFusion(source),
            FileSinkError::InvalidInput { message } => DagError::Schedule(message),
        }
    }
}

pub struct FileSinkNode {
    meta: NodePorts,
    path: String,
    format: WriteFormat,
    mode: SinkMode,
    runtime_env: Arc<RuntimeEnv>,
}

impl FileSinkNode {
    pub fn new(
        path: String,
        format: WriteFormat,
        mode: SinkMode,
        runtime_env: Arc<RuntimeEnv>,
    ) -> Self {
        Self {
            meta: port_layout(),
            path,
            format,
            mode,
            runtime_env,
        }
    }

    /// The file path this sink writes to.
    pub fn sink_path(&self) -> &str {
        &self.path
    }

    /// Whether this sink appends to, or overwrites, the destination.
    pub fn mode(&self) -> SinkMode {
        self.mode
    }

    /// Return the rows already stored at `path` concatenated with `new`, used
    /// to implement true single-file append.
    ///
    /// DataFusion's single-file sink always replaces the target file, so an
    /// append is realized by reading the current contents back, casting each
    /// column to `new`'s schema (so the schemas line up for `union`), and
    /// emitting one combined [`DataFrame`] that is then written with
    /// [`InsertOp::Overwrite`]. If the destination does not yet exist, `new`
    /// is returned unchanged.
    async fn append_existing(
        &self,
        path: &str,
        format: WriteFormat,
        new: DataFrame,
    ) -> Result<DataFrame, FileSinkError> {
        use datafusion::logical_expr::cast;
        use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, col};

        if !std::path::Path::new(path).exists() {
            return Ok(new);
        }

        let read_err = |e: datafusion::error::DataFusionError| FileSinkError::Write {
            path: path.to_string(),
            source: e,
        };
        let ctx = new_isolated_ctx(self.runtime_env.clone(), None);
        let existing = match format {
            WriteFormat::Csv => ctx
                .read_csv(path, CsvReadOptions::default())
                .await
                .map_err(read_err)?,
            WriteFormat::Parquet => ctx
                .read_parquet(path, ParquetReadOptions::default())
                .await
                .map_err(read_err)?,
        };

        // Cast each existing column to the new DataFrame's field type so the
        // two schemas are union-compatible. This matters most for CSV, where
        // integers re-read back as `Int64` regardless of how they were
        // written.
        let target = new.schema().inner();
        let cast_exprs: Vec<_> = target
            .fields()
            .iter()
            .map(|f| cast(col(f.name()), f.data_type().clone()))
            .collect();
        let existing = existing.select(cast_exprs).map_err(read_err)?;
        existing.union(new).map_err(read_err)
    }
}

#[derive(Debug, JsonSchema, Deserialize)]
pub struct FileSinkNodeSpec {
    pub path: String,
    pub format: WriteFormat,
    #[serde(default)]
    pub mode: SinkMode,
}

pub struct FileSinkNodeFactory {}

/// Static port layout for every [`FileSinkNode`]: a single untyped input port
/// and no outputs.
fn port_layout() -> NodePorts {
    NodePorts::new().add_input_port(None)
}

impl NodeFactory for FileSinkNodeFactory {
    fn kind(&self) -> &'static str {
        "sink_file"
    }

    fn desc(&self) -> &'static str {
        "Writes an upstream DataFrame to a file (CSV/Parquet)."
    }

    fn doc(&self) -> &'static str {
        "A file sink node that consumes an upstream DataFrame and writes it to \
        a local/remote file in CSV or Parquet format. Supports both append and \
        overwrite modes. One untyped input port; no output ports."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileSinkNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let node_spec: FileSinkNodeSpec = serde_json::from_value(spec)?;
        let node = FileSinkNode::new(
            node_spec.path,
            node_spec.format,
            node_spec.mode,
            node_ctx.runtime_env,
        );
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for FileSinkNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        let cp_node = Self {
            meta: self.meta.clone(),
            path: self.path.clone(),
            format: self.format,
            mode: self.mode,
            runtime_env: self.runtime_env.clone(),
        };

        Box::new(cp_node)
    }

    fn kind(&self) -> &'static str {
        "sink_file"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(FileSinkError::InvalidInput {
            message: "FileSinkNode requires exactly one upstream input".to_string(),
        })?;

        let path = normalize_path(&self.path);
        let format = self.format;
        let df = input.data.clone();

        // Resolve the DataFrame to actually write. DataFusion's
        // `write_csv`/`write_parquet` do not implement
        // `InsertOp::Overwrite` and their single-file sink always
        // *replaces* the target — so an overwrite is "drop the
        // existing file then write", and an append is "read the
        // existing rows back, merge them, then write".
        let to_write = match self.mode {
            SinkMode::Overwrite => {
                let _ = std::fs::remove_file(&path);
                df
            }
            SinkMode::Append => self.append_existing(&path, format, df).await?,
        };

        let options = DataFrameWriteOptions::new().with_single_file_output(true);

        let res = match format {
            WriteFormat::Csv => to_write.write_csv(&path, options, None::<CsvOptions>).await,
            WriteFormat::Parquet => {
                to_write
                    .write_parquet(&path, options, None::<TableParquetOptions>)
                    .await
            }
        };
        res.map_err(|e| FileSinkError::Write {
            path: path.clone(),
            source: e,
        })?;

        Ok(HashMap::new())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Int32Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::{DataFrame, SessionContext};

    use crate::nodes::{
        FileSinkNode, SinkMode, WriteFormat,
        meta::{DagNode, NodeInput},
    };

    /// Build a small in-memory [`DataFrame`] for sink tests.
    ///
    /// Two columns, three rows — enough to round-trip through both CSV and
    /// Parquet writers without bloating the test runtime. Mirrors the helper
    /// style used in `sql_node::tests::setup_test_node`.
    #[allow(dead_code)]
    fn sample_dataframe() -> (SessionContext, DataFrame) {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["alice", "bob", "carol"])),
            ],
        )
        .expect("sample RecordBatch should construct");
        let df = ctx
            .read_batch(batch)
            .expect("ctx should accept sample batch");
        (ctx, df)
    }

    /// A fresh DataFrame whose rows differ from [`sample_dataframe`] so that
    /// append vs. overwrite is distinguishable by reading the file back.
    fn second_dataframe() -> (SessionContext, DataFrame) {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![4, 5])),
                Arc::new(StringArray::from(vec!["dave", "eve"])),
            ],
        )
        .expect("second RecordBatch should construct");
        let df = ctx
            .read_batch(batch)
            .expect("ctx should accept second batch");
        (ctx, df)
    }

    /// Read the `id` column of a CSV file back as a sorted `Vec<i32>`.
    ///
    /// DataFusion infers integer CSV columns as `Int64`, so we downcast to
    /// `Int64Array` regardless of how the value was originally typed.
    async fn read_csv_ids(ctx: &SessionContext, path: &str) -> Vec<i32> {
        use arrow_array::Int64Array;
        use datafusion::prelude::CsvReadOptions;
        let mut ids: Vec<i32> = ctx
            .read_csv(path, CsvReadOptions::default())
            .await
            .expect("read back sink output")
            .select(vec![datafusion::prelude::col("id")])
            .expect("select id")
            .collect()
            .await
            .expect("collect ids")
            .into_iter()
            .flat_map(|b| {
                b.column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .expect("id is Int64")
                    .iter()
                    .map(|v| v.expect("non-null id") as i32)
                    .collect::<Vec<_>>()
            })
            .collect();
        ids.sort();
        ids
    }

    /// `Overwrite` replaces the destination file entirely.
    #[tokio::test]
    async fn test_sink_file_overwrite_replaces() {
        let ctx = SessionContext::new();
        let runtime_env = ctx.runtime_env();
        let path = format!("/tmp/sink_overwrite_{}.csv", std::process::id());

        let sink = |df: DataFrame, mode| {
            let mut node =
                FileSinkNode::new(path.clone(), WriteFormat::Csv, mode, runtime_env.clone());
            async move {
                node.execute(
                    &[NodeInput { port: 0, data: df }],
                    &crate::dag::node_event::NodeReporter::noop(),
                )
                .await
            }
        };

        sink(sample_dataframe().1, SinkMode::Overwrite)
            .await
            .unwrap();
        sink(second_dataframe().1, SinkMode::Overwrite)
            .await
            .unwrap();

        let ids = read_csv_ids(&ctx, &path).await;
        assert_eq!(ids, vec![4, 5], "overwrite must keep only the second write");
        let _ = std::fs::remove_file(&path);
    }

    /// `Append` stacks successive writes onto the destination file.
    #[tokio::test]
    async fn test_sink_file_append_accumulates() {
        let ctx = SessionContext::new();
        let runtime_env = ctx.runtime_env();
        let path = format!("/tmp/sink_append_{}.csv", std::process::id());

        let write = |df: DataFrame| {
            let mut node = FileSinkNode::new(
                path.clone(),
                WriteFormat::Csv,
                SinkMode::Append,
                runtime_env.clone(),
            );
            async move {
                node.execute(
                    &[NodeInput { port: 0, data: df }],
                    &crate::dag::node_event::NodeReporter::noop(),
                )
                .await
            }
        };

        write(sample_dataframe().1).await.unwrap();
        write(second_dataframe().1).await.unwrap();

        let ids = read_csv_ids(&ctx, &path).await;
        assert_eq!(
            ids,
            vec![1, 2, 3, 4, 5],
            "append must keep rows from both writes"
        );
        let _ = std::fs::remove_file(&path);
    }
}
