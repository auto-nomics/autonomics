use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Open a new tab and make it the active one.
#[tool(
    name = "browser_new_tab",
    description = "Open a new browser tab. If `url` is given, navigate to it before \
                  returning. The new tab becomes the active one; subsequent tools \
                  that omit `tab_index` will operate on it. Returns the new tab's \
                  index."
)]
pub struct NewTabInput {
    #[desc = "Optional URL to navigate the new tab to. Bare hostnames get `https://` prepended."]
    pub url: Option<String>,
}

pub struct NewTabTool {
    session: Arc<BrowserSession>,
}

impl NewTabTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for NewTabTool {
    type Input = NewTabInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let index = match input.url {
            Some(url) => self.session.new_tab_at(&url).await?,
            None => self.session.new_tab().await?,
        };
        Ok(ToolResult::success(index.to_string()))
    }
}
