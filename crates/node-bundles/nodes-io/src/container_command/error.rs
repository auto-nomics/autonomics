//! Errors raised by container-command nodes.
//!
//! The authoritative conversion is [`ContainerCommandError::into_dag_error`],
//! which `execute` calls with the node's real kind so wrapper kinds report
//! themselves instead of the generic `container_command` marker.

use dag_core::dag::DagError;
use thiserror::Error;

use super::CONTAINER_COMMAND_KIND;
use super::utils::capture_preview;
use container_runtime::ContainerRuntimeError;

/// Tail length kept for captured stdout/stderr previews in failure messages.
pub(crate) const FAILURE_CAPTURE_PREVIEW_CHARS: usize = 1000;
/// Tail size kept when inlining declared textual outputs into failures.
pub(crate) const FAILURE_OUTPUT_PREVIEW_BYTES: u64 = 8 * 1024;

#[derive(Debug, Error)]
pub enum ContainerCommandError {
    #[error("invalid container_command spec: {0}")]
    Invalid(String),
    #[error(transparent)]
    Runtime(#[from] ContainerRuntimeError),
    #[error(
        "container exited with status {exit_code}; stderr: {stderr}; stdout: {stdout}; declared output logs: {output_logs:?}"
    )]
    ExitStatus {
        exit_code: i32,
        stderr: String,
        stdout: String,
        output_logs: Vec<(String, String)>,
    },
    #[error("declared output `{path}` was not produced")]
    MissingOutput { path: String },
}

impl ContainerCommandError {
    pub(super) fn diagnostic_message(&self) -> String {
        let Self::ExitStatus {
            exit_code,
            stderr,
            stdout,
            output_logs,
        } = self
        else {
            return self.to_string();
        };

        let mut message = format!("container exited with status {exit_code}");
        if stderr.trim().is_empty() && stdout.trim().is_empty() && output_logs.is_empty() {
            message.push_str("; no stdout, stderr, or declared output logs captured");
            return message;
        }
        if !stderr.trim().is_empty() {
            message.push_str("; ");
            message.push_str(&capture_preview("stderr", stderr));
        }
        if !stdout.trim().is_empty() {
            message.push_str("; ");
            message.push_str(&capture_preview("stdout", stdout));
        }
        for (label, capture) in output_logs {
            message.push_str("; ");
            message.push_str(&capture_preview(label, capture));
        }
        message
    }

    /// Convert into a [`DagError`] carrying the node's real kind. Wrapper
    /// nodes built with their own kind report it here instead of the generic
    /// `container_command` marker.
    pub(super) fn into_dag_error(self, kind: &'static str) -> DagError {
        DagError::NodeError {
            node_type: kind.to_string(),
            msg: self.diagnostic_message(),
        }
    }
}

impl dag_core::dag::NodeError for ContainerCommandError {
    // Generic kind for the blanket `From` path. The authoritative funnel is
    // [`ContainerCommandError::into_dag_error`], which `execute` calls with
    // the node's real kind; this impl only serves conversions that lack that
    // context.
    fn node_type(&self) -> &str {
        CONTAINER_COMMAND_KIND
    }
}
