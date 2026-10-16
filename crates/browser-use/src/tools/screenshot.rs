use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, ToolResultBlock, ToolResultContent};
use async_trait::async_trait;

use crate::browser::BrowserSession;
use crate::tools::tool_err;

/// Capture a PNG screenshot of the active tab and return it as a base64 image block.
#[tool(
    name = "browser_screenshot",
    description = "Capture a PNG screenshot of the current viewport. The result is an \
                  image content block the model can see directly — preferred over \
                  `browser_extract_text` when layout, formatting, or visual content \
                  matter. Returns base64-encoded PNG bytes."
)]
pub struct ScreenshotInput {
    /// Zero-based tab index; defaults to the active tab.
    #[default = 0]
    pub tab_index: Option<usize>,
}

pub struct ScreenshotTool {
    session: Arc<BrowserSession>,
}

impl ScreenshotTool {
    pub fn new(session: Arc<BrowserSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl ToolFunction for ScreenshotTool {
    type Input = ScreenshotInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let tab = self
            .session
            .tab(input.tab_index)
            .await
            ?;
        let bytes = tab.screenshot().await?;
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(ToolResult {
            tool_use_id: String::new(),
            content: ToolResultContent::Blocks(vec![ToolResultBlock::Image {
                source: agentik_sdk::types::ToolImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: encoded,
                },
            }]),
            is_error: None,
        })
    }
}
