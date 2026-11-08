//! Account-scoped HPC tools. Authentication and filesystem authorization remain
//! with OpenSSH and the remote operating system; these tools grant no new roles.

mod followup;
pub mod jobs;
pub mod monitor;
pub mod ssh;

#[cfg(test)]
mod helper_tests;

pub use ssh::{HpcClient, HpcConfig};

use agentik_core::tools::ToolRegistration;
use std::sync::Arc;

/// Reject a common model encoding mistake without rewriting user payloads or
/// silently dropping replacement/ownership constraints.
pub(crate) fn reject_placeholder_strings(
    input: &serde_json::Value,
    fields: &[&str],
) -> Result<(), agentik_core::tools::ToolError> {
    for field in fields {
        if input
            .get(*field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.is_empty() || value == "null")
        {
            return Err(agentik_core::tools::ToolError::ValidationFailed {
                message: format!(
                    "{field} must not be an empty string or the string \"null\"; omit unused optional fields or use JSON null. No remote operation was attempted."
                ),
            });
        }
    }
    Ok(())
}

pub fn registrations(client: Arc<HpcClient>) -> Vec<ToolRegistration> {
    // Scheduler duties only: cluster inspection and job lifecycle. File
    // access moved to the VFS mount layer (explicit domain partitions).
    jobs::registrations(client)
}
