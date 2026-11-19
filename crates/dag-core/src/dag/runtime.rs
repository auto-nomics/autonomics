//! Runtime types shared by the flow scheduler and the executor stack.
//!
//! The legacy node-graph scheduler these types once served was removed
//! (flows are the only executable path); what remains is the execution
//! evidence a node reports about its own run and the serializable error
//! summary used across the wire.

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

pub use super::status::RuntimeStatus;

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

/// Execution-level evidence a node reports about its own run, beyond what the
/// scheduler can observe generically.
///
/// Populated via
/// [`crate::dag::node_event::NodeReporter::set_run_details`] and carried to
/// the terminal `JobResult` — so it survives both the success and the failure
/// path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeRunDetails {
    /// Measured software evidence from the actual remote execution path.
    /// Absent on historical runs. Placement pins and heartbeats cannot fill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_software: Option<serde_json::Value>,
    /// Independently observed scheduler termination, separate from software
    /// identity and payload success. Never populated from an output file alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_stop: Option<serde_json::Value>,
    /// Local or remote task workspace retained for debugging and resume.
    pub workspace: Option<String>,
    /// Machine-readable task manifest containing process identity and I/O contracts.
    pub task_manifest: Option<crate::value::FileRef>,
    /// Full container image reference used for the execution, if any.
    pub image: Option<String>,
    /// The `sha256:` digest parsed from `image`, when the reference is
    /// digest-pinned.
    pub image_digest: Option<String>,
    /// Process exit code (`0` on the success path).
    pub exit_code: Option<i32>,
    /// Resolved in-container executable and arguments submitted to the runtime.
    /// Environment values are deliberately excluded; credentials belong there.
    /// Absent on historical records or when execution was never attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_argv: Option<Vec<String>>,
    /// Per-execution identity shared with the run's artifact directory (for
    /// container nodes: the container name, which also names the scratch
    /// directory and the `{artifact_prefix}/{run_name}/` output subtree).
    pub run_name: Option<String>,
    /// Persisted stdout capture (`local://` URI + sha256), when non-empty.
    pub stdout_log: Option<crate::value::FileRef>,
    /// Persisted stderr capture (`local://` URI + sha256), when non-empty.
    pub stderr_log: Option<crate::value::FileRef>,
    /// File-backed outputs produced by the executor.
    ///
    /// DataFrame outputs are materialized as Arrow IPC files; Channel outputs
    /// are materialized as JSON. This is the cross-executor transfer contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_artifacts: Vec<crate::value::FileRef>,
    /// Output artifacts grouped by the originating output port.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_artifacts_by_port: BTreeMap<u8, Vec<crate::value::FileRef>>,
}
