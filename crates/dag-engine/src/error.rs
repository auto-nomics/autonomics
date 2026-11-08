pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by authored-state construction and validation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A structural rule of the authored graph was violated. The message is
    /// user-facing (agent-facing) and states the exact offending element.
    #[error("flow error: {0}")]
    Flow(#[from] crate::model::Error),
    /// Run-level failure raised while binding or executing through the
    /// dag-core executor stack (bind_run, preflight, node build).
    #[error("flow execution error: {0}")]
    Execution(String),
}
