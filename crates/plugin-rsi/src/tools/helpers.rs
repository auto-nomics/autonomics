//! Cross-tool helpers: error mapping and target resolution.

use agentik_core::tools::ToolError;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::Result as RsiResult;

use super::{PluginTarget, PluginToolState};

pub(super) fn tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: error.to_string().into(),
    }
}

/// Deserialize a structured tool input while retaining its JSON field path.
pub(super) fn parse_tool_input<Input>(input: Value) -> Result<Input, ToolError>
where
    Input: DeserializeOwned,
{
    serde_path_to_error::deserialize(input).map_err(|error| ToolError::ValidationFailed {
        message: format!("invalid input at {}: {error}", error.path()),
    })
}

pub(super) async fn resolve_target(
    state: &PluginToolState,
    plugin_path: &str,
) -> RsiResult<PluginTarget> {
    state.registry.resolve_target(&state.principal, plugin_path)
}
