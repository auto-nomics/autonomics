use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Get the current `<title>` of a tab.
#[tool(
    name = "browser_get_title",
    description = "Return the page's `<title>` text. Empty when the tab has no title \
                  yet (e.g. it is still loading or on `about:blank`)."
)]
pub struct GetTitleInput {
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct GetTitleTool {
    session: Arc<BrowserSession>,
}

impl GetTitleTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for GetTitleTool {
    type Input = GetTitleInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self
            .session
            .tab(input.tab_index)
            .await
            ?;
        let title = tab.title().await?;
        Ok(ToolResult::success(title.unwrap_or_default()))
    }
}
