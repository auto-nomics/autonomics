//! Agent-facing tools that drive a [`BrowserSession`].
//!
//! Each tool is a small struct holding an `Arc<BrowserSession>`; the
//! session is the shared state, and every tool re-locks it briefly to
//! resolve a `Tab` (cheap-clone `Arc<PageInner>`), then operates on the
//! unlocked tab. The lock is never held across an `.await`.
//!
//! Build a registry with [`registrations`].

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::BrowserSession;
use crate::error::Error;

pub mod click;
pub mod close_tab;
pub mod download;
pub mod evaluate;
pub mod extract_html;
pub mod extract_text;
pub mod get_title;
pub mod get_url;
pub mod list_tabs;
pub mod navigate;
pub mod new_tab;
pub mod screenshot;
pub mod scroll;
pub mod switch_tab;
pub mod type_text;
pub mod wait_for_selector;

/// Convert a [`crate::error::Error`] into a [`ToolError::ExecutionFailed`].
///
/// `?` on the `run` body uses this so tool implementations stay flat.
pub(crate) fn tool_err(err: Error) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: Box::new(err),
    }
}

/// Wrap `Error` so it can flow through `?` from a `run` body.
impl From<Error> for agentik_core::tools::ToolError {
    fn from(err: Error) -> Self {
        tool_err(err)
    }
}

/// Build the default set of browser tools, all sharing one session.
pub fn registrations(session: Arc<BrowserSession>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(navigate::NavigateTool::new(session.clone())),
        ToolRegistration::from(click::ClickTool::new(session.clone())),
        ToolRegistration::from(type_text::TypeTextTool::new(session.clone())),
        ToolRegistration::from(extract_text::ExtractTextTool::new(session.clone())),
        ToolRegistration::from(extract_html::ExtractHtmlTool::new(session.clone())),
        ToolRegistration::from(screenshot::ScreenshotTool::new(session.clone())),
        ToolRegistration::from(evaluate::EvaluateTool::new(session.clone())),
        ToolRegistration::from(wait_for_selector::WaitForSelectorTool::new(session.clone())),
        ToolRegistration::from(download::DownloadTool::new(session.clone())),
        ToolRegistration::from(download::WaitForDownloadTool::new(session.clone())),
        ToolRegistration::from(download::ListDownloadsTool::new(session.clone())),
        ToolRegistration::from(get_url::GetUrlTool::new(session.clone())),
        ToolRegistration::from(get_title::GetTitleTool::new(session.clone())),
        ToolRegistration::from(scroll::ScrollTool::new(session.clone())),
        ToolRegistration::from(new_tab::NewTabTool::new(session.clone())),
        ToolRegistration::from(close_tab::CloseTabTool::new(session.clone())),
        ToolRegistration::from(switch_tab::SwitchTabTool::new(session.clone())),
        ToolRegistration::from(list_tabs::ListTabsTool::new(session)),
    ]
}

#[cfg(test)]
mod tests {
    use agentik_sdk::types::ToolInput;
    use serde_json::Value;

    /// Field descriptions must land in the generated JSON Schema — the
    /// `#[desc]` attributes are the only channel the `#[tool]` macro reads
    /// (doc comments are stripped before schemars derives).
    #[test]
    fn field_descriptions_reach_the_schema() {
        let def = super::navigate::NavigateInput::definition();
        let props = &def.input_schema.properties;

        let url_desc = props["url"]["description"].as_str().unwrap_or_default();
        assert!(
            url_desc.contains("https://"),
            "url description missing: {url_desc}"
        );

        let tab_desc = props["tab_index"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            tab_desc.contains("Zero-based"),
            "tab_index description missing: {tab_desc}"
        );
        assert_eq!(props["tab_index"]["default"], serde_json::json!(0));

        let download = super::download::DownloadInput::definition();
        let download_props = &download.input_schema.properties;
        assert!(
            download_props["url"]["description"]
                .as_str()
                .unwrap_or_default()
                .contains("https://")
        );
        assert_eq!(
            download_props["timeout_ms"]["default"],
            serde_json::json!(120000)
        );

        let wait = super::download::WaitForDownloadInput::definition();
        assert!(wait.input_schema.properties.contains_key("timeout_ms"));
    }
}
