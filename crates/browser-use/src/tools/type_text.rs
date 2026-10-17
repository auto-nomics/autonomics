use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::browser::BrowserSession;

/// Click an element and type text into it.
#[tool(
    name = "browser_type_text",
    description = "Focus the first element matching a CSS selector, set its value \
                  using the native DOM setter, and dispatch input/change events. \
                  Use browser_evaluate for form submission or special keys."
)]
pub struct TypeTextInput {
    #[desc = "CSS selector of the input element."]
    pub selector: String,
    #[desc = "Text to enter into the field."]
    pub text: String,
    #[desc = "Zero-based tab index; defaults to the active tab."]
    #[default = 0]
    pub tab_index: Option<usize>,
    #[desc = "Maximum time to wait for the selector to appear, in milliseconds."]
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
        tab.type_text_with_timeout(&input.selector, &input.text, timeout)
            .await?;
        Ok(ToolResult::success(format!(
            "typed {} chars into `{}`",
            input.text.chars().count(),
            input.selector
        )))
    }

    fn timeout_seconds(&self) -> u64 {
        15
    }
}
