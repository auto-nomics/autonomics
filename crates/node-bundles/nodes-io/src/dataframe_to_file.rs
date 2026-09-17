//! DataFrame-to-file bridge node.
//!
//! One DataFrame input port and one File output port. Symmetric to
//! [`crate::file_to_dataframe::FileToDataFrameNode`] across the DataFrame/file boundary.

use arrow_array::Array;
use arrow_cast::cast;
use async_trait::async_trait;
use datafusion::{
    common::HashMap,
    common::config::{CsvOptions, TableParquetOptions},
    dataframe::{DataFrame, DataFrameWriteOptions},
    execution::cache::TableScopedPath,
};
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::file_to_dataframe::{normalize_path, source_path};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::sink::SinkMode;
use dag_core::{
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
    Tsv,
    Parquet,
    Mtx,
}

impl WriteFormat {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Parquet => "parquet",
            Self::Mtx => "mtx",
        }
    }
}

#[derive(Debug, Error)]
pub enum DataFrameToFileError {
    #[error("Invalid input: {message}")]
    InvalidInput { message: String },
    #[error("write sink '{path}' failed")]
    Write {
        path: String,
        #[source]
        source: datafusion::error::DataFusionError,
    },
}

impl DataFrameToFileError {
    pub fn to_dag_error(self) -> DagError {
        match self {
            DataFrameToFileError::Write { source, .. } => DagError::DataFusion(source),
            DataFrameToFileError::InvalidInput { message } => DagError::Schedule(message),
        }
    }
}

impl ::dag_core::dag::NodeError for DataFrameToFileError {
    fn node_type(&self) -> &str {
        "dataframe_to_file"
    }
}

pub struct DataFrameToFileNode {
    meta: NodePorts,
    path: String,
    format: WriteFormat,
    mode: SinkMode,
    partition_by: Vec<String>,
    has_header: bool,
    id_column: String,
}

impl DataFrameToFileNode {
    pub fn new(path: String, format: WriteFormat, mode: SinkMode) -> Self {
        Self::new_with_partitions(path, format, mode, Vec::new())
    }

    pub fn new_with_partitions(
        path: String,
        format: WriteFormat,
        mode: SinkMode,
        partition_by: Vec<String>,
    ) -> Self {
        Self::new_with_options(path, format, mode, partition_by, true, String::new())
    }

    pub fn new_with_options(
        path: String,
        format: WriteFormat,
        mode: SinkMode,
        partition_by: Vec<String>,
        has_header: bool,
        id_column: String,
    ) -> Self {
        Self {
            meta: port_layout(),
            path,
            format,
            mode,
            partition_by,
            has_header,
            id_column,
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

    /// Remove a local directory (or stale file) before a partitioned overwrite.
    fn clear_local_output(path: &str) -> Result<(), DataFrameToFileError> {
        let local_path = path.strip_prefix("file://").unwrap_or(path);
        match std::fs::metadata(local_path) {
            Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(local_path).map_err(|e| {
                DataFrameToFileError::InvalidInput {
                    message: format!("cannot clear partitioned output `{path}`: {e}"),
                }
            }),
            Ok(metadata) if metadata.is_file() => {
                std::fs::remove_file(local_path).map_err(|e| DataFrameToFileError::InvalidInput {
                    message: format!("cannot clear partitioned output `{path}`: {e}"),
                })
            }
            Ok(_) => Err(DataFrameToFileError::InvalidInput {
                message: format!("partitioned output `{path}` is not a directory or file"),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(DataFrameToFileError::InvalidInput {
                message: format!("cannot stat partitioned output `{path}`: {err}"),
            }),
        }
    }

    /// Remove existing objects below a mounted VFS path. Directories can remain;
    /// deleting every object makes a subsequent overwrite authoritative.
    async fn clear_vfs_output(
        node_ctx: &dag_core::registry::NodeCtx,
        path: &str,
    ) -> Result<(), DataFrameToFileError> {
        let Some(virtual_path) = path.strip_prefix("vfs://") else {
            return Err(DataFrameToFileError::InvalidInput {
                message: format!("expected a vfs:// partitioned output path, got `{path}`"),
            });
        };
        let Some(storage) = node_ctx.opendal.as_ref() else {
            return Err(DataFrameToFileError::InvalidInput {
                message: "partitioned VFS output requires registered object storage".into(),
            });
        };
        let op = storage.resolve(virtual_path);
        let root = storage.resolve_path(virtual_path);
        match op.stat(&root).await {
            Err(err) if err.kind() == vfs::opendal::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(DataFrameToFileError::InvalidInput {
                    message: format!("cannot stat partitioned output `{path}`: {err}"),
                });
            }
            Ok(_) => {}
        }

        let mut lister = op.lister_with(&root).recursive(true).await.map_err(|err| {
            DataFrameToFileError::InvalidInput {
                message: format!("cannot list partitioned output `{path}`: {err}"),
            }
        })?;
        while let Some(entry) = lister.next().await {
            let entry = entry.map_err(|err| DataFrameToFileError::InvalidInput {
                message: format!("cannot list partitioned output `{path}`: {err}"),
            })?;
            if entry.metadata().is_file() {
                op.delete(entry.path()).await.map_err(|err| {
                    DataFrameToFileError::InvalidInput {
                        message: format!(
                            "cannot delete partitioned output object `{}`: {err}",
                            entry.path()
                        ),
                    }
                })?;
            }
        }
        Ok(())
    }

    fn invalidate_listing_cache(node_ctx: &dag_core::registry::NodeCtx, path: &str) {
        let Some(cache) = node_ctx
            .session()
            .runtime_env()
            .cache_manager
            .get_list_files_cache()
        else {
            return;
        };
        let virtual_path = path
            .strip_prefix("vfs://")
            .unwrap_or(path)
            .trim_matches('/');
        let Ok(object_path) = datafusion::object_store::path::Path::parse(virtual_path) else {
            return;
        };
        let key = TableScopedPath {
            table: None,
            path: object_path,
        };
        let _ = cache.remove(&key);
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

    async fn write_bytes(
        node_ctx: &dag_core::registry::NodeCtx,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<(), DataFrameToFileError> {
        if let Some(virtual_path) = path.strip_prefix("vfs://") {
            let storage = node_ctx
                .opendal
                .as_ref()
                .ok_or(DataFrameToFileError::InvalidInput {
                    message: format!("VFS output `{path}` requires mounted object storage"),
                })?;
            let operator = storage.resolve(virtual_path);
            operator
                .write(&storage.resolve_path(virtual_path), bytes)
                .await
                .map_err(|error| DataFrameToFileError::InvalidInput {
                    message: format!("cannot write VFS MatrixMarket `{path}`: {error}"),
                })?;
            return Ok(());
        }
        if let Some(parent) = std::path::Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                DataFrameToFileError::InvalidInput {
                    message: format!("cannot create `{}`: {error}", parent.display()),
                }
            })?;
        }
        tokio::fs::write(path, bytes)
            .await
            .map_err(|error| DataFrameToFileError::InvalidInput {
                message: format!("cannot write MatrixMarket `{path}`: {error}"),
            })
    }

    async fn matrix_market_bytes(
        frame: &DataFrame,
        id_column: &str,
    ) -> Result<Vec<u8>, DataFrameToFileError> {
        let batches =
            frame
                .clone()
                .collect()
                .await
                .map_err(|error| DataFrameToFileError::Write {
                    path: "MatrixMarket".into(),
                    source: error,
                })?;
        let fields = frame.schema().fields();
        let id_index = if id_column.is_empty() {
            None
        } else {
            Some(fields.find(id_column).map(|(index, _)| index).ok_or(
                DataFrameToFileError::InvalidInput {
                    message: format!("MatrixMarket id column `{id_column}` is absent"),
                },
            )?)
        };
        let value_indexes = (0..fields.len())
            .filter(|index| Some(*index) != id_index)
            .collect::<Vec<_>>();
        let rows = batches
            .iter()
            .map(arrow_array::RecordBatch::num_rows)
            .sum::<usize>();
        let mut output = format!(
            "%%MatrixMarket matrix coordinate real general\n{} {} 0\n",
            rows,
            value_indexes.len()
        )
        .into_bytes();
        let mut row_offset = 1_usize;
        for batch in &batches {
            for row_index in 0..batch.num_rows() {
                let matrix_row = row_offset + row_index;
                for (column_index, field_index) in value_indexes.iter().enumerate() {
                    let value = cast(batch.column(*field_index), &arrow_schema::DataType::Float64)
                        .map_err(|error| DataFrameToFileError::InvalidInput {
                            message: format!(
                                "MatrixMarket column `{}` is not numeric: {error}",
                                fields[*field_index].name()
                            ),
                        })?;
                    let value = value
                        .as_any()
                        .downcast_ref::<arrow_array::Float64Array>()
                        .ok_or_else(|| DataFrameToFileError::InvalidInput {
                            message: "numeric cast did not produce Float64".into(),
                        })?
                        .value(row_index);
                    if value != 0.0 && value.is_finite() {
                        output.extend_from_slice(
                            format!("{} {} {}\n", matrix_row, column_index + 1, value).as_bytes(),
                        );
                    }
                }
            }
            row_offset += batch.num_rows();
        }
        let nonzero = output
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            .saturating_sub(2);
        let text = String::from_utf8_lossy(&output).into_owned();
        let mut rewritten = String::new();
        rewritten.push_str(text.lines().next().unwrap_or_default());
        rewritten.push('\n');
        rewritten.push_str(&format!("{} {} {}\n", rows, value_indexes.len(), nonzero));
        for line in text.lines().skip(2) {
            rewritten.push_str(line);
            rewritten.push('\n');
        }
        Ok(rewritten.into_bytes())
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
    ) -> Result<DataFrame, DataFrameToFileError> {
        use datafusion::logical_expr::cast;
        use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, col};

        if !self.object_exists(node_ctx, path).await {
            return Ok(new);
        }

        let read_err = |e: datafusion::error::DataFusionError| DataFrameToFileError::Write {
            path: path.to_string(),
            source: e,
        };
        let ctx = node_ctx.session();
        let existing = match format {
            WriteFormat::Csv => ctx
                .read_csv(path, CsvReadOptions::default().has_header(self.has_header))
                .await
                .map_err(read_err)?,
            WriteFormat::Tsv => ctx
                .read_csv(
                    path,
                    CsvReadOptions::default()
                        .delimiter(b'\t')
                        .file_extension("tsv")
                        .has_header(self.has_header),
                )
                .await
                .map_err(read_err)?,
            WriteFormat::Parquet => ctx
                .read_parquet(path, ParquetReadOptions::default())
                .await
                .map_err(read_err)?,
            WriteFormat::Mtx => {
                return Err(DataFrameToFileError::InvalidInput {
                    message: "MatrixMarket output supports overwrite only".into(),
                });
            }
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
pub struct DataFrameToFileNodeSpec {
    pub path: String,
    pub format: WriteFormat,
    #[serde(default)]
    pub mode: SinkMode,
    /// Hive-style columns used to partition Parquet output.
    #[serde(default)]
    pub partition_by: Vec<String>,
    /// Whether CSV/TSV output includes a header row. MatrixMarket ignores it.
    #[serde(default = "default_true")]
    pub has_header: bool,
    /// Row identifier column excluded from MatrixMarket values. Empty means use every column.
    #[serde(default)]
    pub id_column: String,
}

fn default_true() -> bool {
    true
}

pub struct DataFrameToFileNodeFactory {}

/// Static port layout for every [`DataFrameToFileNode`]: one DataFrame input and one
/// File output.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for DataFrameToFileNodeFactory {
    fn kind(&self) -> &'static str {
        "dataframe_to_file"
    }

    fn desc(&self) -> &'static str {
        "Writes an upstream DataFrame to CSV/TSV/Parquet or a coordinate MatrixMarket file."
    }

    fn doc(&self) -> &'static str {
        "A bridge node that consumes an upstream DataFrame and writes it to \
        a local/remote CSV, TSV, Parquet, or coordinate MatrixMarket file. \
        CSV/TSV support append and overwrite with has_header; MatrixMarket is \
        overwrite-only and excludes id_column from numeric values."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DataFrameToFileNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: DataFrameToFileNodeSpec = serde_json::from_value(spec)?;
        let node = DataFrameToFileNode::new_with_options(
            node_spec.path,
            node_spec.format,
            node_spec.mode,
            node_spec.partition_by,
            node_spec.has_header,
            node_spec.id_column,
        );
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for DataFrameToFileNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        let cp_node = Self {
            meta: self.meta.clone(),
            path: self.path.clone(),
            format: self.format,
            partition_by: self.partition_by.clone(),
            mode: self.mode,
            has_header: self.has_header,
            id_column: self.id_column.clone(),
        };

        Box::new(cp_node)
    }

    fn kind(&self) -> &'static str {
        "dataframe_to_file"
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
        let input = inputs.first().ok_or(DataFrameToFileError::InvalidInput {
            message: "DataFrameToFileNode requires exactly one upstream input".to_string(),
        })?;

        let format = self.format;
        let partitioned = !self.partition_by.is_empty();
        if partitioned && format != WriteFormat::Parquet {
            return Err(DataFrameToFileError::InvalidInput {
                message: "partition_by is only supported for Parquet output".to_string(),
            }
            .into());
        }
        let mut path = source_path(node_ctx, &normalize_path(&self.path));
        if partitioned {
            path = path.trim_end_matches('/').to_string();
            path.push('/');
        }
        let df = input
            .dataframe()
            .map_err(|e| DataFrameToFileError::InvalidInput {
                message: e.to_string(),
            })?
            .clone();

        if format == WriteFormat::Mtx {
            if self.mode == SinkMode::Append {
                return Err(DataFrameToFileError::InvalidInput {
                    message: "MatrixMarket output supports overwrite only".into(),
                }
                .into());
            }
            let bytes = Self::matrix_market_bytes(&df, &self.id_column).await?;
            Self::write_bytes(node_ctx, &path, bytes).await?;
            let file = FileRef::local(&path, Some("mtx".into()))
                .unwrap_or_else(|_| FileRef::new(path.clone(), Some("mtx".into())));
            let mut outputs = PortOutputs::new();
            outputs.insert_file(0, file);
            return Ok(outputs);
        }

        // Resolve the DataFrame to actually write. DataFusion's
        // `write_csv`/`write_parquet` do not implement
        // `InsertOp::Overwrite` and their single-file sink always
        // *replaces* the target on completion. Keep the old object visible
        // while the replacement is written; deleting it first would expose a
        // missing-file window to concurrent readers.
        let to_write = if partitioned {
            if self.mode == SinkMode::Overwrite {
                if path.starts_with("vfs://") {
                    Self::clear_vfs_output(node_ctx, &path).await?;
                } else if path.starts_with('/') || path.starts_with("file://") {
                    Self::clear_local_output(&path)?;
                } else {
                    return Err(DataFrameToFileError::InvalidInput {
                        message: format!(
                            "partitioned Parquet overwrite supports local or vfs:// paths, got `{path}`"
                        ),
                    }
                    .into());
                }
            }
            df
        } else {
            match self.mode {
                SinkMode::Overwrite => df,
                SinkMode::Append => self.append_existing(node_ctx, &path, format, df).await?,
            }
        };

        let options = if partitioned {
            DataFrameWriteOptions::new()
                .with_single_file_output(false)
                .with_partition_by(self.partition_by.clone())
        } else {
            DataFrameWriteOptions::new().with_single_file_output(true)
        };

        let csv_options = if matches!(format, WriteFormat::Csv | WriteFormat::Tsv) {
            let mut options = CsvOptions::default();
            options.delimiter = if format == WriteFormat::Tsv {
                b'\t'
            } else {
                b','
            };
            options.has_header = Some(self.has_header);
            Some(options)
        } else {
            None
        };

        let res = match format {
            WriteFormat::Csv | WriteFormat::Tsv => {
                to_write.write_csv(&path, options, csv_options).await
            }
            WriteFormat::Parquet => {
                to_write
                    .write_parquet(&path, options, None::<TableParquetOptions>)
                    .await
            }
            WriteFormat::Mtx => {
                unreachable!("MatrixMarket output is handled before DataFusion sinks")
            }
        };
        res.map_err(|e| DataFrameToFileError::Write {
            path: path.clone(),
            source: e,
        })?;
        if partitioned {
            Self::invalidate_listing_cache(node_ctx, &path);
        }

        let format_label = format.as_label().to_string();
        let file = if partitioned {
            FileRef::new(path.clone(), Some(format_label))
        } else {
            FileRef::local(&path, Some(format_label.clone()))
                .unwrap_or_else(|_| FileRef::new(path.clone(), Some(format_label)))
        };
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }
    use std::sync::Arc;

    use arrow_array::{Int32Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::{DataFrame, SessionContext};
    use vfs::{MountedObjectStore, OpendalFileStorage, VfsManifest};

    use crate::dataframe_to_file::{DataFrameToFileNode, WriteFormat};
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

    fn partitioned_dataframe(ctx: &SessionContext, rows: &[(i32, &str)]) -> DataFrame {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("part", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from_iter(rows.iter().map(|(id, _)| *id))),
                Arc::new(StringArray::from_iter(
                    rows.iter().map(|(_, part)| Some(*part)),
                )),
            ],
        )
        .unwrap();
        ctx.read_batch(batch).unwrap()
    }

    fn matrix_dataframe(ctx: &SessionContext) -> DataFrame {
        let schema = Arc::new(Schema::new(vec![
            Field::new("gene", DataType::Utf8, false),
            Field::new("cell_1", DataType::Int32, false),
            Field::new("cell_2", DataType::Int32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["GENE1", "GENE2"])),
                Arc::new(Int32Array::from(vec![1, 0])),
                Arc::new(Int32Array::from(vec![2, 4])),
            ],
        )
        .unwrap();
        ctx.read_batch(batch).unwrap()
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
    async fn test_dataframe_to_file_overwrite_replaces() {
        let ctx = SessionContext::new();
        let path = format!("/tmp/sink_overwrite_{}.csv", std::process::id());

        let sink = |df: DataFrame, mode| {
            let mut node = DataFrameToFileNode::new(path.clone(), WriteFormat::Csv, mode);
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
    async fn test_dataframe_to_file_append_accumulates() {
        let ctx = SessionContext::new();
        let path = format!("/tmp/sink_append_{}.csv", std::process::id());

        let write = |df: DataFrame| {
            let mut node =
                DataFrameToFileNode::new(path.clone(), WriteFormat::Csv, SinkMode::Append);
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

    /// Explicit TSV output uses tab delimiters and remains append-capable.
    #[tokio::test]
    async fn test_dataframe_to_file_writes_and_appends_tsv() {
        let format: WriteFormat =
            serde_json::from_value(serde_json::json!("tsv")).expect("parse tsv format");
        assert_eq!(format, WriteFormat::Tsv);

        let ctx = SessionContext::new();
        let path = format!("/tmp/sink_tsv_{}.tsv", std::process::id());

        let write = |df: DataFrame| {
            let mut node =
                DataFrameToFileNode::new(path.clone(), WriteFormat::Tsv, SinkMode::Append);
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
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents
                .lines()
                .all(|line| line.contains('\t') && !line.contains(',')),
            "TSV output should use tabs: {contents:?}"
        );

        write(second_dataframe().1).await.unwrap();
        let mut ids: Vec<i32> = ctx
            .read_csv(
                &path,
                datafusion::prelude::CsvReadOptions::default()
                    .delimiter(b'\t')
                    .file_extension("tsv"),
            )
            .await
            .unwrap()
            .select(vec![datafusion::prelude::col("id")])
            .unwrap()
            .collect()
            .await
            .unwrap()
            .into_iter()
            .flat_map(|batch| {
                use arrow_array::Int64Array;
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .iter()
                    .map(|value| value.unwrap() as i32)
                    .collect::<Vec<_>>()
            })
            .collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_dataframe_to_file_writes_coordinate_mtx() {
        let ctx = SessionContext::new();
        let path = format!("/tmp/sink_mtx_{}.mtx", std::process::id());
        let mut sink = DataFrameToFileNode::new_with_options(
            path.clone(),
            WriteFormat::Mtx,
            SinkMode::Overwrite,
            Vec::new(),
            false,
            "gene".into(),
        );
        sink.execute(
            &node_ctx(),
            &[NodeInput::new_dataframe(0, matrix_dataframe(&ctx))],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("%%MatrixMarket matrix coordinate real general\n"));
        assert!(contents.contains("2 2 3\n"));
        assert!(contents.contains("1 1 1\n"));
        assert!(contents.contains("2 2 4\n"));
        let _ = std::fs::remove_file(&path);
    }

    /// An absolute path covered by a VFS mount must write to that backend,
    /// not to the host filesystem at the same textual path.
    #[tokio::test]
    async fn dataframe_to_file_routes_mounted_absolute_paths_through_vfs() {
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
        let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), Some(storage));

        let mut sink =
            DataFrameToFileNode::new("/out.csv".into(), WriteFormat::Csv, SinkMode::Overwrite);
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
        let mut sink =
            DataFrameToFileNode::new("/out.csv".into(), WriteFormat::Csv, SinkMode::Append);
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

    /// Partitioned Parquet writes and reads round-trip through one VFS mount,
    /// including overwrite cleanup and append behavior.
    #[tokio::test]
    async fn dataframe_to_file_round_trips_partitioned_parquet_through_vfs() {
        use datafusion::prelude::{col, lit};

        let backend_root = tempfile::tempdir().unwrap();
        let data_root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(backend_root.path().to_string_lossy().to_string());
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data_root.path(), mounted));
        let ctx = SessionContext::new();
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
            storage.clone(),
        );
        let node_ctx =
            dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), Some(storage.clone()));

        let first = partitioned_dataframe(&ctx, &[(1, "a")]);
        let mut sink = DataFrameToFileNode::new_with_partitions(
            "/out".into(),
            WriteFormat::Parquet,
            SinkMode::Overwrite,
            vec!["part".into()],
        );
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, first)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let second = partitioned_dataframe(&ctx, &[(2, "b")]);
        let mut sink = DataFrameToFileNode::new_with_partitions(
            "/out".into(),
            WriteFormat::Parquet,
            SinkMode::Overwrite,
            vec!["part".into()],
        );
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, second)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let mut source = crate::file_to_dataframe::FileToDataFrameNode::new_with_partitions(
            Some("/out".into()),
            Some(crate::file_to_dataframe::FileFormat::Parquet),
            vec!["part".into()],
        );
        let outputs = source
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.dataframe(0).unwrap();
        assert_eq!(df.clone().count().await.unwrap(), 1);
        assert_eq!(
            df.clone()
                .filter(col("part").eq(lit("b")))
                .unwrap()
                .count()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            df.clone()
                .filter(col("part").eq(lit("a")))
                .unwrap()
                .count()
                .await
                .unwrap(),
            0
        );
        let third = partitioned_dataframe(&ctx, &[(3, "c")]);
        let mut sink = DataFrameToFileNode::new_with_partitions(
            "/out".into(),
            WriteFormat::Parquet,
            SinkMode::Append,
            vec!["part".into()],
        );
        sink.execute(
            &node_ctx,
            &[NodeInput::new_dataframe(0, third)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        fn parquet_files(path: &std::path::Path) -> Vec<String> {
            let mut files = Vec::new();
            let mut stack = vec![path.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let entry = entry.unwrap();
                    if entry.path().is_dir() {
                        stack.push(entry.path());
                    } else if entry.path().extension().is_some_and(|ext| ext == "parquet") {
                        files.push(entry.path().to_string_lossy().to_string());
                    }
                }
            }
            files
        }
        let files = parquet_files(backend_root.path());
        assert_eq!(files.len(), 2, "{files:?}");

        let outputs = source
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.dataframe(0).unwrap();
        assert_eq!(df.clone().count().await.unwrap(), 2);
    }

    /// DataFusion's Parquet sink buffers output through multipart writes. This
    /// regression covers objects well above the default multipart threshold.
    #[tokio::test]
    async fn dataframe_to_file_writes_large_parquet_through_vfs() {
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
        let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), Some(storage));

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
        let mut sink = DataFrameToFileNode::new(
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

        let mut sink = DataFrameToFileNode::new(
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
