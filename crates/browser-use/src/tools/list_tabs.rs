use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// List all open tabs with their URL, title, and which one is active.
#[tool(
    name = "browser_list_tabs",
    description = "Return a JSON array of open tabs. Each entry has `index`, `url`, \
                  `title`, and `active` (true for the current active tab). Use this \
                  to discover the right `tab_index` before calling the other tools."
)]
pub struct ListTabsInput {}

pub struct ListTabsTool {
    session: Arc<BrowserSession>,
}

impl ListTabsTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ListTabsTool {
    type Input = ListTabsInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let tabs = self.session.list_tabs().await?;
        let active = self.session.active_index().await;
        let arr: Vec<serde_json::Value> = tabs
            .into_iter()
            .enumerate()
            .map(|(index, info)| {
                serde_json::json!({
                    "index": index,
                    "url": info.url,
                    "title": info.title,
                    "active": index == active,
                })
            })
            .collect();
        Ok(ToolResult::success_json(serde_json::Value::Array(arr)))
    }
}
