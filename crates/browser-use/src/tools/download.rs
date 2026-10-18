use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agentik_core::tools::{
    ExecutionMode, ProgressRecord, ToolContext, ToolError, ToolFunction,
};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use serde_json::Value;

use crate::browser::BrowserSession;

/// How often the background download reports byte progress to the task log.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Download a URL in a browser tab.
///
/// Runs as a background task — Chromium downloads routinely outlast a single
/// turn, so the toolset returns a placeholder immediately and the agent pulls
/// the result with `wait_task` / `view_task_results` while
/// `view_task_status` reports live byte progress.
#[tool(
    name = "browser_download",
    description = "Download a URL in a browser tab and wait until Chromium reports completion or \
                  cancellation. Returns the local file and `/browser-downloads/...` VFS path. \
                  Runs in the background: the call returns a task number straight away — poll \
                  `view_task_status` for progress and use `wait_task` or `view_task_results` \
                  with that number to collect the result."
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

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Async
    }

    /// Hard ceiling for the background task. The tool's own `timeout_ms`
    /// (120s default) bounds the wait first; this is only the backstop that
    /// stops a wedged Chromium from pinning the task forever.
    fn timeout_seconds(&self) -> u64 {
        900
    }

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

    /// Same as [`run`](Self::run), plus a background-progress reporter so
    /// `view_task_status` shows byte counts while the download is in flight.
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Overriding `execute_with_context` bypasses the default
        // `execute → run` chain, so deserialize by hand.
        let input: DownloadInput = serde_json::from_value(input)?;
        let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(120_000));

        ctx.emit(
            ProgressRecord::new("status")
                .status("downloading")
                .message(format!("requesting {}", input.url)),
        );

        // The session tracks byte progress in its own download map; a
        // poller mirrors the newest entry onto the task's live log. It stops
        // as soon as `download` returns, whatever the outcome.
        let done = Arc::new(AtomicBool::new(false));
        let poller = {
            let session = self.session.clone();
            let done = Arc::clone(&done);
            let ctx = ctx.clone();
            tokio::spawn(async move {
                while !done.load(Ordering::Relaxed) {
                    tokio::time::sleep(PROGRESS_POLL_INTERVAL).await;
                    for info in session.download_infos().await {
                        if info.state == "canceled" || info.state == "completed" {
                            continue;
                        }
                        ctx.emit(
                            ProgressRecord::new("progress")
                                .label(info.suggested_filename.clone())
                                .status(info.state.clone())
                                .current(info.received_bytes as u64)
                                .total(if info.total_bytes > 0.0 {
                                    info.total_bytes as u64
                                } else {
                                    0
                                }),
                        );
                    }
                }
            })
        };

        let result = self
            .session
            .download(&input.url, input.tab_index, timeout)
            .await;
        done.store(true, Ordering::Relaxed);
        let _ = poller.await;

        match result {
            Ok(info) => {
                ctx.emit(
                    ProgressRecord::new("finished")
                        .status("completed")
                        .label(info.suggested_filename.clone())
                        .message(info.vfs_path.clone().unwrap_or_default()),
                );
                Ok(ToolResult::success(
                    serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            Err(err) => {
                let message = err.to_string();
                ctx.emit(
                    ProgressRecord::new("finished")
                        .status("failed")
                        .level("error")
                        .message(message.clone()),
                );
                Err(err.into())
            }
        }
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
