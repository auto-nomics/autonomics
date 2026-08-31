//! Runtime types for DAG execution.
//!
//! The actual scheduling logic lives in [`super::graph::DAG::run`]; this module
//! defines the types it produces and accepts.

use datafusion::common::HashMap;
use serde::Serialize;

use super::NodeId;
use super::error::DagError;

/// Schema map serializable via serde (uses `std::collections::HashMap` since
/// `datafusion::common::HashMap` is `hashbrown` and may not impl `Serialize`).
type SchemaMap = std::collections::HashMap<String, String>;

/// Above this column count an output schema is reported in *folded* form
/// (leading columns + type distribution) rather than verbatim, so a single
/// wide table cannot dominate the run report.
const SCHEMA_FULL_THRESHOLD: usize = 50;
/// Number of leading columns retained when a schema is folded. Deliberately
/// small — the type distribution carries the shape, these are just a sample
/// so the agent can see concrete column names / types.
const SCHEMA_PREVIEW_COLS: usize = 20;

/// Compact, agent-facing summary of a node's output schema.
///
/// Narrow schemas (≤ [`SCHEMA_FULL_THRESHOLD`] columns) are reported in full.
/// Wider schemas are *folded*: only the leading [`SCHEMA_PREVIEW_COLS`] columns
/// are listed and a type distribution is attached, so the agent can still gauge
/// the table's shape (e.g. "1000 Utf8, 47 Float64") without a per-column dump
/// flooding the report. [`Self::column_count`] is always the true total.
#[derive(Debug, Serialize)]
pub struct SchemaReport {
    /// Total column count of the output (always the real number, even when
    /// `columns` is folded).
    pub column_count: usize,
    /// `{column_name: data_type}` for the leading columns. Complete when
    /// `column_count <= SCHEMA_FULL_THRESHOLD`; otherwise the first
    /// [`SCHEMA_PREVIEW_COLS`] entries — a sample, not the full set.
    pub columns: SchemaMap,
    /// `{data_type: count}` tallied across **all** columns. `None` for narrow
    /// schemas (where `columns` is already exhaustive); `Some` when the schema
    /// was folded.
    pub type_distribution: Option<std::collections::HashMap<String, usize>>,
}

impl SchemaReport {
    /// Build a report from an Arrow field list, folding wide schemas per the
    /// thresholds above.
    pub fn from_fields(fields: &arrow_schema::Fields) -> Self {
        let column_count = fields.len();
        let folded = column_count > SCHEMA_FULL_THRESHOLD;
        let take = if folded {
            SCHEMA_PREVIEW_COLS
        } else {
            column_count
        };

        let mut columns = SchemaMap::with_capacity(take);
        for f in fields.iter().take(take) {
            columns.insert(f.name().clone(), f.data_type().to_string());
        }

        let type_distribution = if folded {
            let mut dist: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for f in fields.iter() {
                *dist.entry(f.data_type().to_string()).or_insert(0) += 1;
            }
            Some(dist)
        } else {
            None
        };

        SchemaReport {
            column_count,
            columns,
            type_distribution,
        }
    }
}

/// Per-node runtime lifecycle state tracked by the scheduler.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Success,
    Failed,
    /// Not run because an upstream predecessor failed.
    Skipped,
}

/// Per-node dirty-mark state for incremental execution.
///
/// A node is `Dirty` when its spec, payload, wiring, or an upstream output has
/// changed since its last successful run. An incremental `run` (see
/// [`super::SchedulerConfig::incremental`]) skips `Clean` nodes and reuses
/// their cached outputs instead of re-executing them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirtyState {
    /// Output is cached and up-to-date. An incremental run skips this node.
    #[default]
    Clean,
    /// Needs re-execution. Set by mutations and propagated to all transitive
    /// descendants.
    Dirty,
}

/// Scheduler tuning knobs.
#[derive(Clone)]
pub struct SchedulerConfig {
    /// Maximum number of nodes running concurrently (semaphore permits).
    pub max_concurrency: usize,
    /// When `true`, [`NodeReport::output_rows`] is populated by forcing every
    /// successful node's `DataFrame` to be collected (i.e. `SELECT COUNT(*)`
    /// over the LogicalPlan). This is an **eager** operation — for a source
    /// node reading `.vcf.gz` or similar, it triggers full decompression and
    /// parsing of every record, dominating the run cost.
    ///
    /// Defaults to `false` so that `run_dag` remains a "logical only" call:
    /// `SourceNode`/`SqlNode` execute lazily, the report reflects timings and
    /// schema without doing the I/O. Enable explicitly when downstream tooling
    /// (agents, dashboards, callers) needs row counts.
    pub compute_row_counts: bool,
    /// When `true`, `run` only re-executes nodes marked [`DirtyState::Dirty`]
    /// and skips `Clean` nodes whose cached outputs are retained from a
    /// previous successful run. When `false` (default), every node is
    /// re-executed unconditionally (current behavior).
    ///
    /// Mutations (`replace_node`, `add_edge`, `delete_edge`, …) automatically
    /// mark affected nodes and their transitive descendants dirty. Callers can
    /// also manually mark nodes dirty via [`crate::dag::DAG::mark_dirty`] —
    /// useful when an external input (file or VFS dataset) has changed.
    pub incremental: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            max_concurrency: cpus,
            compute_row_counts: false,
            incremental: false,
        }
    }
}

/// A serializable error summary extracted from [`DagError`].
///
/// Carries only the agent-relevant fields (kind + message) rather than the
/// full structured variant, because [`datafusion::error::DataFusionError`]
/// does not implement `Serialize`.
#[derive(Debug, Clone, Serialize)]
pub struct DagErrorReport {
    /// Error variant category (e.g. `"datafusion"`, `"schema_mismatch"`,
    /// `"node_error"`).
    pub kind: String,
    /// Full human-readable error message.
    pub message: String,
}

/// Per-node execution summary produced by [`super::graph::DAG::run`].
///
/// Contains everything an agent needs to understand what each node did
/// without a follow-up `get_output` call — type, output shape, timing,
/// sink destination, and error/skip details.
#[derive(Debug, Serialize)]
pub struct NodeReport {
    pub id: String,
    pub status: RuntimeStatus,
    pub node_type: String,
    /// Payload type of the first output value, when the node produced output.
    pub output_type: Option<String>,
    /// File outputs carried by File and FileSet values.
    pub output_files: Vec<crate::value::FileRef>,
    /// Output column schema. Narrow schemas list every column; wide schemas
    /// are folded to a leading-column sample + type distribution (see
    /// [`SchemaReport`]).
    pub output_schema: Option<SchemaReport>,
    /// Row count of the primary output DataFrame.
    pub output_rows: Option<usize>,
    /// Milliseconds spent in `execute()`.
    pub elapsed_ms: Option<u64>,

    /// For `VizNode` (and future artifact-producing nodes): the path of the
    /// rendered/produced artifact (e.g. a PNG).
    pub artifact_path: Option<String>,
    /// For `dataframe_to_file` nodes: the file path data was written to.
    pub file_path: Option<String>,
    /// Structured error info when `status` is `Failed`.
    pub error: Option<DagErrorReport>,
    /// For `Skipped` nodes: the id of the root-cause failed node.
    pub skipped_because: Option<String>,
}

/// Result of a `DAG::run` invocation: the final status of every node and
/// whether the whole run succeeded.
#[derive(Debug)]
pub struct RunReport {
    pub ok: bool,
    /// Non-fatal runtime warnings, such as a successful DAG run whose
    /// history snapshot could not be persisted.
    pub warnings: Vec<String>,
    /// Snapshot id committed after this run. `None` when history is absent,
    /// the snapshot commit failed, or an unchanged manifest was skipped.
    pub snapshot_id: Option<String>,
    /// Rich per-node reports (serializable, agent-friendly).
    pub nodes: Vec<NodeReport>,
    /// Flat status map kept for backward-compatible programmatic access.
    pub statuses: HashMap<NodeId, RuntimeStatus>,
    /// Per-node errors (only populated for `Failed` nodes).
    pub errors: HashMap<NodeId, DagError>,
}

impl Serialize for RunReport {
    fn serialize<S: serde::ser::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("RunReport", 6)?;
        st.serialize_field("ok", &self.ok)?;
        st.serialize_field("warnings", &self.warnings)?;
        st.serialize_field("snapshot_id", &self.snapshot_id)?;
        st.serialize_field("nodes", &self.nodes)?;

        // Convert hashbrown HashMaps to std HashMaps for serialization.
        let statuses: std::collections::HashMap<&str, RuntimeStatus> = self
            .statuses
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        st.serialize_field("statuses", &statuses)?;

        let errors: std::collections::HashMap<&str, DagErrorReport> = self
            .errors
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_report()))
            .collect();
        st.serialize_field("errors", &errors)?;
        st.end()
    }
}

impl RunReport {
    pub fn status(&self, id: &str) -> Option<RuntimeStatus> {
        self.statuses.get(id).copied()
    }

    /// The error that failed `id`, if any.
    pub fn error(&self, id: &str) -> Option<&DagError> {
        self.errors.get(id)
    }
}

/// Target scope for a DAG run (reserved for future partial / sub-graph
/// execution).
pub enum DagRunTarget {
    StartWith,
    EndWith,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_schema::{DataType, Field, Fields};

    use super::*;

    fn fields(n: usize, ty: DataType) -> Fields {
        let vec: Vec<Arc<Field>> = (0..n)
            .map(|i| Arc::new(Field::new(format!("c{i}"), ty.clone(), true)))
            .collect();
        vec.into()
    }

    #[test]
    fn narrow_schema_reported_in_full() {
        // 10 columns — under the threshold, so every column is listed and no
        // type distribution is attached.
        let report = SchemaReport::from_fields(&fields(10, DataType::Int32));
        assert_eq!(report.column_count, 10);
        assert_eq!(report.columns.len(), 10, "all columns retained");
        assert!(
            report.type_distribution.is_none(),
            "narrow schemas must not carry a type distribution"
        );
    }

    #[test]
    fn wide_schema_folded_to_preview_plus_distribution() {
        // 200 columns — well over the threshold. Alternate Utf8/Float64 so the
        // type distribution is non-trivial, then overwrite the leading
        // SCHEMA_PREVIEW_COLS with Int32 "c0".."c19" so we can assert exactly
        // which columns survived the fold.
        let mut all: Vec<Arc<Field>> = (0..200)
            .map(|i| {
                let ty = if i % 2 == 0 {
                    DataType::Utf8
                } else {
                    DataType::Float64
                };
                Arc::new(Field::new(format!("x{i}"), ty, true))
            })
            .collect();
        for (i, field) in fields(SCHEMA_PREVIEW_COLS, DataType::Int32)
            .iter()
            .take(SCHEMA_PREVIEW_COLS)
            .cloned()
            .enumerate()
        {
            all[i] = field;
        }
        let all_fields: Fields = all.into();

        let report = SchemaReport::from_fields(&all_fields);
        assert_eq!(report.column_count, 200, "true total preserved");
        assert_eq!(
            report.columns.len(),
            SCHEMA_PREVIEW_COLS,
            "wide schema must be truncated to the preview count"
        );
        // Every retained column must be one of the leading preview columns.
        for name in report.columns.keys() {
            assert!(
                name.starts_with("c") && name.len() <= 4, // "c0".."c19"
                "unexpected retained column {name}"
            );
        }
        let dist = report
            .type_distribution
            .as_ref()
            .expect("wide schema must carry a type distribution");
        // 180 of the 200 columns alternate Utf8/Float64 → 90 each, plus 20
        // Int32 leading columns.
        assert_eq!(dist.get("Int32").copied(), Some(20));
        assert_eq!(dist.get("Utf8").copied(), Some(90));
        assert_eq!(dist.get("Float64").copied(), Some(90));
    }

    #[test]
    fn threshold_boundary_is_full() {
        // Exactly SCHEMA_FULL_THRESHOLD columns is still "narrow" (full).
        let report = SchemaReport::from_fields(&fields(SCHEMA_FULL_THRESHOLD, DataType::Int32));
        assert_eq!(report.columns.len(), SCHEMA_FULL_THRESHOLD);
        assert!(report.type_distribution.is_none());
    }
}
