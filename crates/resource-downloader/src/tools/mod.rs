//! Agent-facing resource downloader tools.
use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::manager::DownloadManager;

mod download;

/// Build the central downloader tool set backed by the process-wide manager.
pub fn registrations(manager: Arc<DownloadManager>) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(download::ResourceDownloadTool::new(
        manager,
    ))]
}
