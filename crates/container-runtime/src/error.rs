use thiserror::Error;

/// Errors raised while validating or invoking the container runtime.
#[derive(Debug, Error)]
pub enum ContainerRuntimeError {
    #[error("invalid container request: {0}")]
    Invalid(String),
    #[error("container data-plane I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("object storage operation failed: {0}")]
    ObjectStorage(String),
    #[error("invalid panel manifest at `{path}`: {message}")]
    PanelManifest { path: String, message: String },
    #[error("container exited with status {exit_code}; stderr: {stderr}; stdout: {stdout}")]
    ExitStatus {
        exit_code: i32,
        stderr: String,
        stdout: String,
    },
    #[error("container was killed before completing within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}
