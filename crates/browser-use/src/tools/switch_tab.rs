use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Switch the active tab.
#[tool(
    name = "browser_switch_tab",
    description = "Make the tab at `tab_index` the active one. Subsequent tools that \
                  omit `tab_index` will operate on it. Returns an error if the index \
                  is out of range."
)]
pub struct SwitchTabInput {
    /// Zero-based index of the tab to activate.
    pub tab_index: usize,
}

pub struct SwitchTabTool {
    session: Arc<BrowserSession>,
}

impl SwitchTabTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for SwitchTabTool {
    type Input = SwitchTabInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.session
            .switch_tab(input.tab_index)
            .await
            ?;
        Ok(ToolResult::success(format!("active tab = {}", input.tab_index)))
    }
}
