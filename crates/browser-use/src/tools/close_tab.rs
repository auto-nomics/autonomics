use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Close a tab and shift the active index if it pointed at or past the closed tab.
#[tool(
    name = "browser_close_tab",
    description = "Close the tab at `tab_index` and return the new number of open tabs. \
                  If the closed tab was at or before the active index, the active index \
                  is shifted down by one (clamped to 0)."
)]
pub struct CloseTabInput {
    #[desc = "Zero-based index of the tab to close. Defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct CloseTabTool {
    session: Arc<BrowserSession>,
}

impl CloseTabTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for CloseTabTool {
    type Input = CloseTabInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let index = match input.tab_index {
            Some(i) => i,
            None => self.session.active_index().await,
        };
        self.session.close_tab(index).await?;
        Ok(ToolResult::success(format!("closed tab {index}")))
    }
}
