//! DAG error types.
//!
//! [`DagError`] is the single error type surfaced by node bodies and the
//! scheduler. Structural failures (cycles, unknown / duplicate node ids,
//! scheduler invariant violations) get their own variants so callers can
//! switch on them; DataFusion errors propagate directly.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DagError>;

#[derive(Debug, Error)]
pub enum DagError {
    /// Propagates any [`datafusion::error::DataFusionError`] that escapes a
    /// node body (e.g. register_table, sql, read_csv).
    #[error("datafusion: {0}")]
    DataFusion(#[from] datafusion::error::DataFusionError),

    /// The graph contains a cycle; carries a representation of the cycle
    /// path (e.g. `A → B → C → A`) for diagnostics.
    #[error("cycle detected: {0}")]
    Cycle(String),

    /// An edge referenced a node id that was never added.
    #[error("unknown node id: {0}")]
    UnknownNode(String),

    #[error("fail to resolve node_id to node_idx")]
    CannotResolveNodeIdx { node_id: String },

    /// `add_node` was called twice with the same id.
    #[error("duplicate node id: {0}")]
    DuplicateNode(String),

    /// An edge referenced an input/output port that the node did not declare.
    /// `direction` is `"input"` or `"output"`.
    #[error("node `{node}` has no {direction} port `{port}`")]
    PortNotFound {
        node: String,
        port: u8,
        direction: &'static str,
    },

    /// A declared input port has no incoming edge (every input port must be
    /// connected).
    #[error("input port `{port}` on node `{node}` is not connected")]
    PortDisconnected { node: String, port: u8 },

    /// More than one edge connects to the same input port (strict 1:1).
    #[error("input port `{port}` on node `{node}` has multiple incoming edges")]
    PortOverconnected { node: String, port: u8 },

    /// Connected ports declare incompatible schemas.
    #[error("schema mismatch on edge {from_node}.{from_port} -> {to_node}.{to_port}: {reason}")]
    SchemaMismatch {
        from_node: String,
        from_port: u8,
        to_node: String,
        to_port: u8,
        reason: String,
    },

    /// Connected ports declare incompatible edge payload types.
    #[error(
        "port type mismatch on edge {from_node}.{from_port} -> {to_node}.{to_port}: expected {expected}, got {actual}"
    )]
    PortTypeMismatch {
        from_node: String,
        from_port: u8,
        to_node: String,
        to_port: u8,
        expected: String,
        actual: String,
    },

    /// A scheduler invariant was violated (e.g. a job result arrived for a node
    /// the scheduler did not dispatch).
    #[error("scheduler: {0}")]
    Schedule(String),

    /// An edge is wired but its predecessor completed without publishing a
    /// value on the connected output port, so the input was never delivered.
    /// Detected at dispatch time — previously the input was dropped silently
    /// and the node executed with missing inputs (e.g. a container starting
    /// without `AUTONOMICS_INPUT0`).
    #[error(
        "edge {from_node}.{from_port} -> {to_node}.{to_port} delivered no value: \
         `{from_node}` finished without a value on output port {from_port} \
         (stale incremental cache or a node that omitted a declared output); \
         re-run with incremental=false or fix the wiring"
    )]
    MissingUpstreamOutput {
        from_node: String,
        from_port: u8,
        to_node: String,
        to_port: u8,
    },

    #[error("node `{node_type}` failed: {msg}")]
    NodeError { node_type: String, msg: String },

    /// No edge exists between the given node–port pair.
    #[error("no edge from `{from}.{from_port}` to `{to}.{to_port}`")]
    EdgeNotFound {
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    },

    /// A snapshot / history operation failed (DB I/O, serialization, etc.).
    #[error("history: {0}")]
    History(String),
}

// ── NodeError trait + blanket From impl ────────────────────────────────────

/// Marker trait for node-specific error types that convert into [`DagError`].
///
/// Node error types impl this trait (providing their `kind` string) and the
/// blanket `From<E: NodeError> for DagError` below handles the conversion.
/// This avoids orphan-rule conflicts: without this trait, a downstream crate
/// cannot `impl From<MyError> for DagError` because neither `From` nor
/// `DagError` is local to that crate.
pub trait NodeError: std::error::Error + Send + Sync + 'static {
    /// The node kind string (e.g. `"hlme"`, `"ldsc_rg"`).
    fn node_type(&self) -> &str;
}

/// Blanket conversion: any [`NodeError`] can be turned into a [`DagError`]
/// via `?` in a node's `execute()` method.
impl<E: NodeError> From<E> for DagError {
    fn from(e: E) -> Self {
        DagError::NodeError {
            node_type: e.node_type().to_string(),
            msg: e.to_string(),
        }
    }
}

/// Maximum number of characters retained in an agent-facing error message.
///
/// Some error variants — chiefly [`DagError::DataFusion`] — can carry
/// multi-kilobyte messages (full SQL text, optimized physical plans, deep
/// cause chains). Stuffing those verbatim into a [`super::runtime::NodeReport`]
/// bloats the run result the agent has to read. We keep a generous head (so
/// the actionable cause stays visible) and drop the tail, marking the cut.
const ERROR_MESSAGE_MAX_CHARS: usize = 1000;

/// Truncate `msg` to [`ERROR_MESSAGE_MAX_CHARS`] characters, appending a
/// marker when content was dropped so the reader knows the message is partial.
fn truncate_message(msg: String) -> String {
    if msg.chars().count() <= ERROR_MESSAGE_MAX_CHARS {
        return msg;
    }
    let head: String = msg.chars().take(ERROR_MESSAGE_MAX_CHARS).collect();
    format!("{head}…(truncated, {} total chars)", msg.chars().count())
}

impl DagError {
    /// Extract a serializable error summary for agent-facing reports.
    ///
    /// The `message` is truncated to [`ERROR_MESSAGE_MAX_CHARS`] characters so
    /// a single bloated error (e.g. a DataFusion plan dump) cannot dominate
    /// the run report.
    pub fn to_report(&self) -> super::runtime::DagErrorReport {
        let (kind, message) = match self {
            Self::DataFusion(e) => ("datafusion", e.to_string()),
            Self::Cycle(s) => ("cycle", s.clone()),
            Self::UnknownNode(s) => ("unknown_node", s.clone()),
            Self::CannotResolveNodeIdx { node_id } => (
                "error_resolve_idx",
                format!(
                    "Error occured when resolve node_id '{node_id}' into graph idx. This may caused by Node didn't registered in graph set properly."
                ),
            ),
            Self::DuplicateNode(s) => ("duplicate_node", s.clone()),
            Self::PortNotFound {
                node,
                port,
                direction,
            } => (
                "port_not_found",
                format!("node `{node}` has no {direction} port `{port}`"),
            ),
            Self::PortDisconnected { node, port } => (
                "port_disconnected",
                format!("input port `{port}` on node `{node}` is not connected"),
            ),
            Self::PortOverconnected { node, port } => (
                "port_overconnected",
                format!("input port `{port}` on node `{node}` has multiple incoming edges"),
            ),
            Self::SchemaMismatch {
                from_node,
                from_port,
                to_node,
                to_port,
                reason,
            } => (
                "schema_mismatch",
                format!("edge {from_node}.{from_port} -> {to_node}.{to_port}: {reason}"),
            ),
            Self::Schedule(s) => ("schedule", s.clone()),
            Self::PortTypeMismatch {
                from_node,
                from_port,
                to_node,
                to_port,
                expected,
                actual,
            } => (
                "port_type_mismatch",
                format!(
                    "edge {from_node}.{from_port} -> {to_node}.{to_port}: expected {expected}, got {actual}"
                ),
            ),
            Self::NodeError { node_type, msg } => {
                ("node_error", format!("node `{node_type}` failed: {msg}"))
            }
            Self::EdgeNotFound {
                from,
                from_port,
                to,
                to_port,
            } => (
                "edge_not_found",
                format!("no edge from `{from}.{from_port}` to `{to}.{to_port}`"),
            ),
            Self::History(msg) => ("history", msg.clone()),
            Self::MissingUpstreamOutput {
                from_node,
                from_port,
                to_node,
                to_port,
            } => (
                "missing_upstream_output",
                format!("edge {from_node}.{from_port} -> {to_node}.{to_port} delivered no value"),
            ),
        };
        super::runtime::DagErrorReport {
            kind: kind.into(),
            message: truncate_message(message),
        }
    }
}
