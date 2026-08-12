//! Error types for the workflow-editor crate.
//!
//! Most modules return [`Result`] / [`WorkflowError`]. The error variants are
//! deliberately split by layer (storage, model, executor, api) so callers can
//! match on the kind without inspecting strings.

use thiserror::Error;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, WorkflowError>;

/// Top-level error type for the workflow-editor crate.
#[derive(Debug, Error)]
pub enum WorkflowError {
    /// A persistence operation failed (sqlx, I/O, migrations, ...).
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    /// A model-level invariant was violated (validation, serialization).
    #[error("model error: {0}")]
    Model(#[from] ModelError),

    /// The executor failed (topology, dispatch, cancellation).
    #[error("executor error: {0}")]
    Executor(#[from] ExecutorError),

    /// The API surface returned an error (unknown workflow, bad cmd).
    #[error("api error: {0}")]
    Api(#[from] ApiError),

    /// Anything else.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<serde_json::Error> for WorkflowError {
    fn from(e: serde_json::Error) -> Self {
        WorkflowError::Model(e.into())
    }
}

impl From<std::io::Error> for WorkflowError {
    fn from(e: std::io::Error) -> Self {
        WorkflowError::Storage(e.into())
    }
}

impl From<rusqlite::Error> for WorkflowError {
    fn from(e: rusqlite::Error) -> Self {
        WorkflowError::Storage(e.into())
    }
}

/// Storage-layer error (rusqlite, I/O, migrations).
#[derive(Debug, Error)]
pub enum StorageError {
    /// rusqlite returned an error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Migration failed (corrupt migration file, checksum mismatch, ...).
    #[error("migration failed: {0}")]
    Migration(String),

    /// Row could not be decoded into the expected Rust type.
    #[error("row decode failed: {0}")]
    Decode(String),

    /// I/O error opening / reading / writing the database file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The requested entity was not found in storage.
    #[error("not found: {kind} id={id}")]
    NotFound {
        /// What was missing (workflow, skill, snapshot, ...).
        kind: &'static str,
        /// Its id (as string).
        id: String,
    },

    /// A uniqueness constraint was violated.
    #[error("conflict: {0}")]
    Conflict(String),
}

/// Model-level error (serde, schema validation, business invariants).
#[derive(Debug, Error)]
pub enum ModelError {
    /// JSON (de)serialization failed.
    #[error("serde_json: {0}")]
    Serde(#[from] serde_json::Error),

    /// Schemars schema validation failed.
    #[error("schema validation: {0}")]
    Schema(String),

    /// The workflow manifest is invalid (cycle, missing port, ...).
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// The skill manifest is invalid.
    #[error("invalid skill: {0}")]
    InvalidSkill(String),

    /// The manifest schema version is unsupported.
    #[error("unsupported schema version: {found} (max supported: {max})")]
    UnsupportedSchemaVersion {
        /// What version we got.
        found: u32,
        /// Highest version we can read.
        max: u32,
    },
}

/// Executor-layer error.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// The DAG has a cycle.
    #[error("workflow contains a cycle")]
    Cycle,

    /// A node's required input port is missing.
    #[error("node {node} missing required input port {port}")]
    MissingInputPort {
        /// Node id (as string).
        node: String,
        /// Port id.
        port: String,
    },

    /// A referenced skill does not exist.
    #[error("skill not found: {0}")]
    SkillNotFound(String),

    /// Node dispatch failed.
    #[error("node {kind} failed: {message}")]
    NodeFailed {
        /// Node kind.
        kind: String,
        /// Failure message.
        message: String,
    },

    /// A tool call outside the skill's `tool_refs` whitelist was attempted.
    #[error("tool call denied: {tool} not in skill whitelist {allowed:?}")]
    ToolDenied {
        /// Tool name that was attempted.
        tool: String,
        /// Whitelist of the enclosing skill.
        allowed: Vec<String>,
    },

    /// The run was cancelled by the caller's `CancellationToken`.
    #[error("execution cancelled")]
    Cancelled,

    /// Internal invariant violated — should not happen.
    #[error("internal: {0}")]
    Internal(String),
}

/// API-layer error (WorkflowClient returned Err).
#[derive(Debug, Error)]
pub enum ApiError {
    /// The actor received a malformed / unsupported command.
    #[error("invalid command: {0}")]
    InvalidCommand(String),

    /// A workflow id was supplied that does not exist.
    #[error("workflow not found: {0}")]
    WorkflowNotFound(uuid::Uuid),

    /// The client lost its actor (channel closed).
    #[error("client disconnected")]
    Disconnected,
}