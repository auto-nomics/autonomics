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

use crate::error::Error;
use crate::BrowserSession;

pub mod click;
pub mod close_tab;
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
        ToolRegistration::from(get_url::GetUrlTool::new(session.clone())),
        ToolRegistration::from(get_title::GetTitleTool::new(session.clone())),
        ToolRegistration::from(scroll::ScrollTool::new(session.clone())),
        ToolRegistration::from(new_tab::NewTabTool::new(session.clone())),
        ToolRegistration::from(close_tab::CloseTabTool::new(session.clone())),
        ToolRegistration::from(switch_tab::SwitchTabTool::new(session.clone())),
        ToolRegistration::from(list_tabs::ListTabsTool::new(session)),
    ]
}
