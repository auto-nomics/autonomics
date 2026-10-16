use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::error::Error;
use crate::tools::tool_err;

/// Click the first element matching a CSS selector, waiting up to
/// `timeout_ms` for it to appear.
#[tool(
    name = "browser_click",
    description = "Click the first element matching a CSS selector. Waits up to \
                  `timeout_ms` (default 10000) for the element to appear. Does NOT \
                  wait for any resulting navigation — use `browser_navigate` again \
                  or call `browser_wait_for_selector` if the page should change."
)]
pub struct ClickInput {
    /// CSS selector of the element to click.
    pub selector: String,
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
    /// Maximum time to wait for the selector to appear, in milliseconds.
    #[default = 10000]
    pub timeout_ms: Option<u64>,
}

pub struct ClickTool {
    session: Arc<BrowserSession>,
}

impl ClickTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ClickTool {
    type Input = ClickInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let timeout = input.timeout_ms.map(std::time::Duration::from_millis);
        tab.wait_for_selector(&input.selector, timeout)
            .await?
            .click()
            .await
            .map_err(Error::from)?;
        Ok(ToolResult::success(format!("clicked `{}`", input.selector)))
    }
}
