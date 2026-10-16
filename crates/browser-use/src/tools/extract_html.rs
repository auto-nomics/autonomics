use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Get the page's full serialized HTML.
#[tool(
    name = "browser_extract_html",
    description = "Return the full serialized DOM (`document.documentElement.outerHTML`). \
                  Use when the agent needs to inspect markup that the visible text \
                  does not expose (attributes, hidden nodes, ARIA roles, etc.)."
)]
pub struct ExtractHtmlInput {
    #[desc = "Zero-based tab index; defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct ExtractHtmlTool {
    session: Arc<BrowserSession>,
}

impl ExtractHtmlTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ExtractHtmlTool {
    type Input = ExtractHtmlInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let html = tab.html().await?;
        Ok(ToolResult::success(html))
    }
}
