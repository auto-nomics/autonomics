use agentik_core::tools::ToolError;

use crate::error::ChemblError;

pub(crate) fn json_err(error: ChemblError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

pub(crate) fn query(limit: Option<u32>, offset: Option<u32>) -> crate::query::ResourceQuery {
    crate::query::ResourceQuery::new()
        .limit(limit.unwrap_or(20).min(100))
        .offset(offset.unwrap_or(0))
}
