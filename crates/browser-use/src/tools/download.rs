use std::sync::Arc;
use std::time::Duration;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;

/// Start a browser download and wait for its final state.
#[tool(
    name = "browser_download",
    description = "Download a URL in a browser tab and wait until Chromium reports completion or \
                  cancellation. Returns the local file and `/browser-downloads/...` VFS path."
)]
pub struct DownloadInput {
    #[desc = "URL to download. `https://` is prepended if the scheme is missing."]
    pub url: String,
    #[desc = "Zero-based index of the tab to use; defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
    #[desc = "Maximum time to wait for a final download state, in milliseconds."]
    #[default = 120000]
    pub timeout_ms: Option<u64>,
}

pub struct DownloadTool {
    session: Arc<BrowserSession>,
}

impl DownloadTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for DownloadTool {
    type Input = DownloadInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(120_000));
        let info = self
            .session
            .download(&input.url, input.tab_index, timeout)
            .await?;
        Ok(ToolResult::success(
            serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()),
        ))
    }
}

/// Wait for a download initiated by a browser click or form submission.
#[tool(
    name = "browser_wait_for_download",
    description = "Wait for a download initiated by `browser_click` or another page action, then \
                  return its local file and `/browser-downloads/...` VFS path."
)]
pub struct WaitForDownloadInput {
    #[desc = "Maximum time to wait for a final download state, in milliseconds."]
    #[default = 120000]
    pub timeout_ms: Option<u64>,
}

pub struct WaitForDownloadTool {
    session: Arc<BrowserSession>,
}

impl WaitForDownloadTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for WaitForDownloadTool {
    type Input = WaitForDownloadInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(120_000));
        let info = self.session.wait_for_download(timeout).await?;
        Ok(ToolResult::success(
            serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()),
        ))
    }
}

/// List downloads observed by this browser session.
#[tool(
    name = "browser_list_downloads",
    description = "List observed browser downloads with state, progress, local path, and VFS path."
)]
pub struct ListDownloadsInput {}

pub struct ListDownloadsTool {
    session: Arc<BrowserSession>,
}

impl ListDownloadsTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ListDownloadsTool {
    type Input = ListDownloadsInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success(
            serde_json::to_string(&self.session.download_infos().await)
                .unwrap_or_else(|_| "[]".to_string()),
        ))
    }
}
