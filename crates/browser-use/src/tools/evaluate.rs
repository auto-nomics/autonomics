use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use serde_json::Value;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Evaluate an arbitrary JavaScript expression in the page and return its JSON value.
#[tool(
    name = "browser_evaluate",
    description = "Evaluate a JavaScript expression in the page context and return its \
                  result as JSON. Use for anything the higher-level tools don't cover: \
                  scraping computed attributes, dispatching events, mutating the DOM, \
                  reading `localStorage`, etc. The expression is retried automatically \
                  across navigation-context swaps. JSON `null` results come back as \
                  `Value::Null`."
)]
pub struct EvaluateInput {
    /// JavaScript expression to evaluate. The return value is serialized to JSON.
    pub expression: String,
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct EvaluateTool {
    session: Arc<BrowserSession>,
}

impl EvaluateTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for EvaluateTool {
    type Input = EvaluateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self
            .session
            .tab(input.tab_index)
            .await
            ?;
        let value: Value = tab.evaluate(&input.expression).await?;
        Ok(ToolResult::success_json(value))
    }
}
