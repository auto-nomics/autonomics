//! File sink node: consumes an upstream `DataFrame` and writes it to a file
//! (CSV or Parquet).
//!
//! One DataFrame input port and one File output port. Symmetric to
//! [`crate::source_file::FileSourceNode`] across the DataFrame/file boundary.

use async_trait::async_trait;
use datafusion::{
    common::HashMap,
    common::config::{CsvOptions, TableParquetOptions},
    dataframe::{DataFrame, DataFrameWriteOptions},
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::source_file::{normalize_path, source_path};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::sink::SinkMode;
use dag_core::{
    codegen::context::{CodegenCtx, CodegenError, NodeCodegen},
    dag::DagError,
    dag::graph::PortOutputs,
    registry::{NodeCtx, NodeFactory},
    value::{FileRef, PortType},
};

/// Supported on-disk write formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WriteFormat {
    Csv,
    Parquet,
}

impl WriteFormat {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Parquet => "parquet",
        }
    }
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

impl FileSinkError {
    pub fn to_dag_error(self) -> DagError {
        match self {
            FileSinkError::Write { source, .. } => DagError::DataFusion(source),
            FileSinkError::InvalidInput { message } => DagError::Schedule(message),
        }
    }
}

impl ::dag_core::dag::NodeError for FileSinkError {
    fn node_type(&self) -> &str {
        "sink_file"
    }
}

pub struct FileSinkNode {
    meta: NodePorts,
    path: String,
    format: WriteFormat,
    mode: SinkMode,
}

impl FileSinkNode {
    pub fn new(path: String, format: WriteFormat, mode: SinkMode) -> Self {
        Self {
            meta: port_layout(),
            path,
            format,
            mode,
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

    /// Check a DataFusion path without relying on host-path semantics. A
    /// `vfs://` object is resolved through the mounted OpenDAL backend.
    async fn object_exists(&self, node_ctx: &dag_core::registry::NodeCtx, path: &str) -> bool {
        let Some(virtual_path) = path.strip_prefix("vfs://") else {
            return std::path::Path::new(path).exists();
        };
        let Some(storage) = node_ctx.opendal.as_ref() else {
            return false;
        };
        storage
            .resolve(virtual_path)
            .stat(&storage.resolve_path(virtual_path))
            .await
            .is_ok()
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
        node_ctx: &dag_core::registry::NodeCtx,
        path: &str,
        format: WriteFormat,
        new: DataFrame,
    ) -> Result<DataFrame, FileSinkError> {
        use datafusion::logical_expr::cast;
        use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, col};

        if !self.object_exists(node_ctx, path).await {
            return Ok(new);
        }

        let read_err = |e: datafusion::error::DataFusionError| FileSinkError::Write {
            path: path.to_string(),
            source: e,
        };
        let ctx = node_ctx.session();
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

/// Static port layout for every [`FileSinkNode`]: one DataFrame input and one
/// File output.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for FileSinkNodeFactory {
    fn kind(&self) -> &'static str {
        "sink_file"
    }

    fn desc(&self) -> &'static str {
        "Writes an upstream DataFrame to a file (CSV/Parquet)."
    }

    fn doc(&self) -> &'static str {
        "A file bridge node that consumes an upstream DataFrame and writes it to \
        a local/remote file in CSV or Parquet format. Supports both append and \
        overwrite modes, and emits the written file on its output port."
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
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: FileSinkNodeSpec = serde_json::from_value(spec)?;
        let node = FileSinkNode::new(node_spec.path, node_spec.format, node_spec.mode);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let node_spec: FileSinkNodeSpec =
            serde_json::from_value(spec.clone()).map_err(|e| CodegenError::BadSpec {
                kind: "sink_file".into(),
                source: e,
            })?;

        let path = &node_spec.path;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let write_call = match node_spec.format {
            WriteFormat::Csv => format!(r#"fwrite({input}, "{path}")"#),
            WriteFormat::Parquet => format!(r#"write_parquet({input}, "{path}")"#),
        };
        let output_var = ctx.output_var.to_string();
        let code = vec![write_call, format!(r#"{output_var} <- "{path}""#)];

        Ok(NodeCodegen {
            code,
            output_vars: vec![output_var],
            extra_packages: vec![],
        })
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
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
        };

        Box::new(cp_node)
    }

    fn kind(&self) -> &'static str {
        "sink_file"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.path)
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(FileSinkError::InvalidInput {
            message: "FileSinkNode requires exactly one upstream input".to_string(),
        })?;

        let path = source_path(node_ctx, &normalize_path(&self.path));
        let format = self.format;
        let df = input
            .dataframe_value()
            .map_err(|e| FileSinkError::InvalidInput {
                message: e.to_string(),
            })?
            .clone();

        // Resolve the DataFrame to actually write. DataFusion's
        // `write_csv`/`write_parquet` do not implement
        // `InsertOp::Overwrite` and their single-file sink always
        // *replaces* the target on completion. Keep the old object visible
        // while the replacement is written; deleting it first would expose a
        // missing-file window to concurrent readers.
        let to_write = match self.mode {
            SinkMode::Overwrite => df,
            SinkMode::Append => self.append_existing(node_ctx, &path, format, df).await?,
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

        let format_label = format.as_label().to_string();
        let file = FileRef::local(&path, Some(format_label.clone()))
            .unwrap_or_else(|_| FileRef::new(path.clone(), Some(format_label)));
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }
    use std::sync::Arc;

    use arrow_array::{Int32Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::{DataFrame, SessionContext};
    use vfs::{MountedObjectStore, OpendalFileStorage, VfsManifest};

    use crate::sink_file::{FileSinkNode, WriteFormat};
    use dag_core::{DagNode, NodeInput, SinkMode};

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
        let path = format!("/tmp/sink_overwrite_{}.csv", std::process::id());

        let sink = |df: DataFrame, mode| {
            let mut node = FileSinkNode::new(path.clone(), WriteFormat::Csv, mode);
            async move {
                node.execute(
                    &node_ctx(),
                    &[NodeInput::new_dataframe(0, df)],
                    &dag_core::dag::node_event::NodeReporter::noop(),
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
        let path = format!("/tmp/sink_append_{}.csv", std::process::id());

        let write = |df: DataFrame| {
            let mut node = FileSinkNode::new(path.clone(), WriteFormat::Csv, SinkMode::Append);
            async move {
                node.execute(
                    &node_ctx(),
                    &[NodeInput::new_dataframe(0, df)],
                    &dag_core::dag::node_event::NodeReporter::noop(),
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

    /// An absolute path covered by a VFS mount must write to that backend,
    /// not to the host filesystem at the same textual path.
    #[tokio::test]
    async fn sink_file_routes_mounted_absolute_paths_through_vfs() {
        let backend_root = tempfile::tempdir().unwrap();
        let data_root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(backend_root.path().to_string_lossy().to_string());
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data_root.path(), mounted));
        let (ctx, first) = sample_dataframe();
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
            storage.clone(),
        );
        let node_ctx = dag_core::registry::NodeCtx {
            runtime_env: ctx.runtime_env().clone(),
            opendal: Some(storage),
            global_sem: None,
        };

        let mut sink = FileSinkNode::new("/out.csv".into(), WriteFormat::Csv, SinkMode::Overwrite);
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, first)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
        assert!(backend_root.path().join("out.csv").exists());

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let second_batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![4, 5])),
                Arc::new(StringArray::from(vec!["dave", "eve"])),
            ],
        )
        .unwrap();
        let second = ctx.read_batch(second_batch).unwrap();
        let mut sink = FileSinkNode::new("/out.csv".into(), WriteFormat::Csv, SinkMode::Append);
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, second)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let ids = read_csv_ids(&ctx, "vfs:///out.csv").await;
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    }

    /// DataFusion's Parquet sink buffers output through multipart writes. This
    /// regression covers objects well above the default multipart threshold.
    #[tokio::test]
    async fn sink_file_writes_large_parquet_through_vfs() {
        use arrow::array::Float64Array;
        use arrow::datatypes::{DataType, Field, Schema};

        fn splitmix64(state: &mut u64) -> u64 {
            *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = *state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_1ebd);
            z ^ (z >> 31)
        }

        let rows = 8_000_000usize;
        let mut state = 0x42u64;
        let x = Float64Array::from_iter((0..rows).map(|_| {
            let bits = splitmix64(&mut state);
            (bits >> 11) as f64 / (1u64 << 53) as f64
        }));
        let mut state = 0x1234u64;
        let y = Float64Array::from_iter((0..rows).map(|_| {
            let bits = splitmix64(&mut state);
            (bits >> 11) as f64 / (1u64 << 53) as f64
        }));

        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let input = ctx
            .read_batch(RecordBatch::try_new(schema, vec![Arc::new(x), Arc::new(y)]).unwrap())
            .unwrap();

        let backend_root = tempfile::tempdir().unwrap();
        let data_root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(backend_root.path().to_string_lossy().to_string());
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data_root.path(), mounted));
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
            storage.clone(),
        );
        let node_ctx = dag_core::registry::NodeCtx {
            runtime_env: ctx.runtime_env().clone(),
            opendal: Some(storage),
            global_sem: None,
        };

        let first = ctx
            .read_batch(
                RecordBatch::try_new(
                    Arc::new(Schema::new(vec![
                        Field::new("x", DataType::Float64, false),
                        Field::new("y", DataType::Float64, false),
                    ])),
                    vec![
                        Arc::new(Float64Array::from(vec![0.5])),
                        Arc::new(Float64Array::from(vec![0.25])),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
        let mut sink = FileSinkNode::new(
            "/large.parquet".into(),
            WriteFormat::Parquet,
            SinkMode::Overwrite,
        );
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, first)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let mut sink = FileSinkNode::new(
            "/large.parquet".into(),
            WriteFormat::Parquet,
            SinkMode::Append,
        );
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, input)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let path = backend_root.path().join("large.parquet");
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(
            metadata.len() > 8 * 1024 * 1024,
            "file size {}",
            metadata.len()
        );
        let output = ctx
            .read_parquet("vfs:///large.parquet", Default::default())
            .await
            .unwrap();
        assert_eq!(output.count().await.unwrap(), rows + 1);
    }
}
