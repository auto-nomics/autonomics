use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Navigate the active tab (or `tab_index`) to `url`.
#[tool(
    name = "browser_navigate",
    description = "Navigate a browser tab to a URL and wait for the document to load. \
                  Bare hostnames (e.g. `example.com`) are upgraded to `https://`. \
                  Returns the final URL the tab landed on."
)]
pub struct NavigateInput {
    /// URL to navigate to. `https://` is prepended if the scheme is missing.
    pub url: String,
    /// Zero-based index of the tab to use; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct NavigateTool {
    session: Arc<BrowserSession>,
}

impl NavigateTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for NavigateTool {
    type Input = NavigateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        tab.goto(&input.url).await?;
        let landed = tab.url().await?;
        Ok(ToolResult::success(landed.unwrap_or_default()))
    }
}
