//! Dedicated database-backed memory tools.

mod list;
mod note;
mod read;
mod search;
mod util;

use std::sync::Arc;

use crate::tools::ToolRegistration;

use super::KmsAgentHandle;
pub use list::{MemoryListInput, MemoryListTool};
pub use note::{MemoryNoteInput, MemoryNoteTool};
pub use read::{MemoryReadInput, MemoryReadTool};
pub use search::{MemorySearchInput, MemorySearchTool};

#[must_use]
pub fn memory_registrations(backend: Arc<KmsAgentHandle>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(MemorySearchTool::new(Arc::clone(&backend))),
        ToolRegistration::from(MemoryReadTool::new(Arc::clone(&backend))),
        ToolRegistration::from(MemoryListTool::new(Arc::clone(&backend))),
        ToolRegistration::from(MemoryNoteTool::new(backend)),
    ]
}
