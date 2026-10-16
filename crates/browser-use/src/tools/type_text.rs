use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::error::Error;
use crate::tools::tool_err;

/// Click an element and type text into it.
#[tool(
    name = "browser_type_text",
    description = "Click the first element matching a CSS selector, then type `text` \
                  into it. Use for form fields and search boxes. Special key \
                  sequences (e.g. Enter) are sent as keystrokes; for key chords \
                  use `browser_press_key` instead."
)]
pub struct TypeTextInput {
    /// CSS selector of the input element.
    pub selector: String,
    /// Text to type. Each character is sent as a separate key event.
    pub text: String,
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
    /// Maximum time to wait for the selector to appear, in milliseconds.
    #[default = 10000]
    pub timeout_ms: Option<u64>,
}

pub struct TypeTextTool {
    session: Arc<BrowserSession>,
}

impl TypeTextTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for TypeTextTool {
    type Input = TypeTextInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self.session.tab(input.tab_index).await?;
        let timeout = input.timeout_ms.map(std::time::Duration::from_millis);
        let element = tab.wait_for_selector(&input.selector, timeout).await?;
        element.click().await.map_err(Error::from)?;
        element.type_str(&input.text).await.map_err(Error::from)?;
        Ok(ToolResult::success(format!(
            "typed {} chars into `{}`",
            input.text.chars().count(),
            input.selector
        )))
    }
}
