use agentik_sdk::AnthropicError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Error)]
pub enum Error {
    #[error("no memory items in the current segment (items vector is empty)")]
    EmptyMemoryItem,

    #[error("failed to compact memory: {0}")]
    Compact(#[from] AnthropicError),

    #[error(
        "orphaned tool_result: no matching tool_use block with id '{tool_use_id}' was found \
         in any message — this usually means the tool_use message was dropped or the \
         tool_result arrived out of order"
    )]
    OrphanToolResult { tool_use_id: String },

    #[error(
        "unexpected message layout: expected a user-role message after the tool_use \
         at index {msg_index} (id '{tool_use_id}'), but found a different role or \
         message structure"
    )]
    UnexpectedMessageLayout { msg_index: usize, tool_use_id: String },
}
