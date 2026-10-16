use std::sync::Arc;
use std::time::Duration;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::error::Error;
use crate::tools::tool_err;

/// Wait for an element matching a CSS selector to appear.
#[tool(
    name = "browser_wait_for_selector",
    description = "Block until an element matching the CSS selector is present in the DOM, \
                  or until `timeout_ms` elapses (default 10000). Returns the matching \
                  element's outerHTML. Use after a click that triggers a render, or to \
                  synchronize on client-rendered content before extracting."
)]
pub struct WaitForSelectorInput {
    #[desc = "CSS selector to wait for."]
    pub selector: String,
    #[desc = "Zero-based tab index; defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
    #[desc = "Maximum wait time in milliseconds."]
    #[default = 10000]
    pub timeout_ms: Option<u64>,
}

pub struct WaitForSelectorTool {
    session: Arc<BrowserSession>,
}

impl WaitForSelectorTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for WaitForSelectorTool {
    type Input = WaitForSelectorInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let timeout = input.timeout_ms.map(Duration::from_millis);
        let element = tab.wait_for_selector(&input.selector, timeout).await?;
        let html = element.outer_html().await.map_err(Error::from)?;
        Ok(ToolResult::success(html.unwrap_or_default()))
    }
}
