use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Scroll the viewport.
#[tool(
    name = "browser_scroll",
    description = "Scroll the viewport by `(x, y)` pixels relative to the current \
                  position (negative values scroll up/left). The page does not \
                  auto-load; for infinite-scroll pages, call repeatedly until the \
                  target content is rendered, then extract."
)]
pub struct ScrollInput {
    /// Horizontal pixels to scroll (negative = left).
    pub x: i32,
    /// Vertical pixels to scroll (negative = up).
    pub y: i32,
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct ScrollTool {
    session: Arc<BrowserSession>,
}

impl ScrollTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ScrollTool {
    type Input = ScrollInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        tab.scroll_by(input.x, input.y).await?;
        Ok(ToolResult::success(format!(
            "scrolled by ({}, {})",
            input.x, input.y
        )))
    }
}
