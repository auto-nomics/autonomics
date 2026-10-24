use agentik_memory::MemoryError;

use crate::tools::ToolError;

pub(super) fn store_error(error: MemoryError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
