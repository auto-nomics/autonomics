//! Status glyph and name-display helpers shared by agent-listing surfaces
//! (status bar, docked agents panel).

use ratatui::style::Color;

use crate::state::AgentStatus;

/// Well-known workspace-root prefix stripped from displayed agent names:
/// every agent's workspace lives under it, so it carries no signal.
const WORKSPACE_ROOT_PREFIX: &str = "/root/";

pub(crate) fn display_name(name: &str) -> &str {
    name.strip_prefix(WORKSPACE_ROOT_PREFIX).unwrap_or(name)
}

pub(crate) fn status_icon(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle | AgentStatus::Aborted => "●",
        AgentStatus::Requesting => "◐",
        AgentStatus::Streaming => "◑",
        AgentStatus::ToolRunning => "⚙",
        AgentStatus::Retrying => "↻",
        AgentStatus::Error => "✗",
        AgentStatus::Cancelled => "⊘",
        AgentStatus::Waiting => "☐",
        AgentStatus::Compacting => "⟳",
    }
}

pub(crate) fn status_color(status: &AgentStatus) -> Color {
    let theme = crate::theme::current();
    match status {
        AgentStatus::Idle | AgentStatus::Aborted => theme.success,
        AgentStatus::Requesting | AgentStatus::Streaming => theme.accent,
        AgentStatus::ToolRunning => theme.warning,
        AgentStatus::Retrying => theme.warning,
        AgentStatus::Error => theme.error,
        AgentStatus::Cancelled => theme.text_faint,
        AgentStatus::Waiting => theme.info,
        AgentStatus::Compacting => theme.accent_alt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_strips_workspace_root_prefix() {
        assert_eq!(display_name("/root/gwas/fig4"), "gwas/fig4");
    }

    #[test]
    fn display_name_keeps_names_without_the_prefix() {
        assert_eq!(display_name("default"), "default");
        // Bare root (no trailing component) is not the prefix.
        assert_eq!(display_name("/root"), "/root");
    }
}
