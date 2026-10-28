use agentik_memory::StoreError;

use crate::tools::ToolError;

pub(super) fn store_error(error: StoreError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
