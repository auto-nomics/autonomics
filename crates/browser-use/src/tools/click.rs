use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::error::Error;
use crate::tools::tool_err;

/// Click the first element matching a CSS selector, waiting up to a fixed
/// 5-second deadline for it to appear.
#[tool(
    name = "browser_click",
    description = "Click the first element matching a CSS selector. Waits up to \
                  5 seconds for the element to appear. Does NOT wait for any \
                  resulting navigation — use `browser_navigate` again or call \
                  `browser_wait_for_selector` if the page should change."
)]
pub struct ClickInput {
    #[desc = "CSS selector of the element to click."]
    pub selector: String,
    #[desc = "Zero-based tab index; defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
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
        tab.wait_for_selector(&input.selector, Some(std::time::Duration::from_secs(5)))
            .await?
            .click()
            .await
            .map_err(Error::from)?;
        Ok(ToolResult::success(format!("clicked `{}`", input.selector)))
    }
}
