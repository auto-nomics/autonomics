use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Get the current URL of a tab.
#[tool(
    name = "browser_get_url",
    description = "Return the current URL of a tab. Empty when the tab is on \
                  `about:blank` or has not yet loaded."
)]
pub struct GetUrlInput {
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct GetUrlTool {
    session: Arc<BrowserSession>,
}

impl GetUrlTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for GetUrlTool {
    type Input = GetUrlInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let url = tab.url().await?;
        Ok(ToolResult::success(url.unwrap_or_default()))
    }
}
