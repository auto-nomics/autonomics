//! Cross-tool helpers: error mapping and target resolution.

use agentik_core::tools::ToolError;

use crate::Result as RsiResult;

use super::{PluginTarget, PluginToolState};

pub(super) fn tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: error.to_string().into(),
    }
}

pub(super) async fn resolve_target(
    state: &PluginToolState,
    plugin_path: &str,
) -> RsiResult<PluginTarget> {
    state.registry.resolve_target(&state.principal, plugin_path)
}
