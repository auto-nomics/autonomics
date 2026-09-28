//! Session-state errors.

use agentik_sdk::AnthropicError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to compact: {0}")]
    Compact(#[from] AnthropicError),

    #[error(
        "orphaned tool_result: no matching tool_use block with id '{tool_use_id}' was found \
         in any message — this usually means the tool_use message was dropped or the \
         tool_result arrived out of order"
    )]
    OrphanToolResult { tool_use_id: String },
}
