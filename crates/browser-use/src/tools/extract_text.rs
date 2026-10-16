use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Get the visible text of the page body.
#[tool(
    name = "browser_extract_text",
    description = "Return the visible text of the page (`document.body.innerText`). \
                  Waits up to 30s for the document to reach `complete`. Long pages \
                  are returned verbatim — for a structured digest use a summarizer \
                  on the result, or query specific subtrees via `browser_evaluate`."
)]
pub struct ExtractTextInput {
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct ExtractTextTool {
    session: Arc<BrowserSession>,
}

impl ExtractTextTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ExtractTextTool {
    type Input = ExtractTextInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let text = tab.text().await?;
        Ok(ToolResult::success(text))
    }
}
