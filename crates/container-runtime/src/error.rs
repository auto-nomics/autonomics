use thiserror::Error;

/// Errors raised while validating or invoking a container backend.
#[derive(Debug, Error)]
pub enum ContainerRuntimeError {
    #[error("invalid container request: {0}")]
    Invalid(String),
    #[error("container command failed to start: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`{command}` exited with status {exit_code}: {stderr}")]
    Command {
        command: &'static str,
        exit_code: i32,
        stderr: String,
    },
    #[error("container exited with status {exit_code}: {stderr}")]
    ExitStatus { exit_code: i32, stderr: String },
    #[error("container was killed before completing within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}
