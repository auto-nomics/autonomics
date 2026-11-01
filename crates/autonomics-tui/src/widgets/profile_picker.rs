//! Profile picker — flat list widget for selecting an agent kind.
//!
//! Two-column layout: a flat list on the left (one row per hardcoded agent
//! kind), and a preview pane on the right showing the selected kind's
//! configuration details. Self-contained (state + rendering + key handling
//! all live here).

use agentik_core::AgentKind;
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::Color,
    widgets::{ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::tree_picker::{
    PickerTreeRow, kind_preview_lines, render_chrome, render_footer, render_preview_frame,
    render_preview_text, render_search, render_separator, render_tree_list, split_content,
    truncate_preview_lines,
};

/// One visible row in the list — a leaf per agent kind.
#[derive(Clone, Debug)]
struct KindRow {
    kind: AgentKind,
}

impl PickerTreeRow for KindRow {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn depth(&self) -> usize {
        0
    }

    fn is_leaf(&self) -> bool {
        true
    }

    fn expanded(&self) -> bool {
        false
    }
}

/// State for the profile picker.
#[derive(Default)]
pub struct ProfilePickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    /// All kinds.
    items: Vec<AgentKind>,
    /// Flat list of visible rows, rebuilt on search changes.
    rows: Vec<KindRow>,
    selected: usize,
    list_state: ListState,
}

impl ProfilePickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.rebuild_rows();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.rebuild_rows();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.rebuild_rows();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.rows.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Kept for key-handler symmetry with the tree pickers; there are no
    /// folders to expand anymore, so this is a no-op.
    pub fn toggle_expand(&mut self) {}

    /// Returns the currently selected agent kind, if any row is selected.
    pub fn selected_item(&self) -> Option<AgentKind> {
        self.rows.get(self.selected).map(|row| row.kind)
    }

    /// Populate the picker from agent kinds.
    pub fn set_profiles(&mut self, profiles: Vec<AgentKind>) {
        self.items = profiles;
        self.rebuild_rows();
    }

    // ── Internal ──

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.rows.is_empty() || self.selected >= self.rows.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }

    /// Rebuild the flat `rows` vector from `items`, honoring the search
    /// filter (matches against the kind name and description).
    fn rebuild_rows(&mut self) {
        let needle = self.query.trim().to_lowercase();
        self.rows = self
            .items
            .iter()
            .filter(|kind| {
                needle.is_empty()
                    || kind.name().to_lowercase().contains(&needle)
                    || kind.description().to_lowercase().contains(&needle)
            })
            .map(|kind| KindRow { kind: *kind })
            .collect();
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.sync_list_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// Standalone widget that renders the profile picker as a two-column popup.
pub struct ProfilePicker {
    pub accent: Color,
    /// Width of the popup (0 = auto, ~80% of frame).
    pub popup_width: u16,
    /// Width of the left list block.
    pub list_width: u16,
}

impl ProfilePicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Cyan,
            popup_width: 0,
            list_width: 28,
        }
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn list_width(mut self, w: u16) -> Self {
        self.list_width = w;
        self
    }
}

impl Default for ProfilePicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for ProfilePicker {
    type State = ProfilePickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ProfilePickerState) {
        if !state.visible {
            return;
        }

        let layout = render_chrome(area, buf, " Select Profile ", self.accent, self.popup_width);
        render_search(layout.search, buf, self.accent, &state.query, "profiles");
        render_separator(layout.separator, buf);

        let content = split_content(layout.content, self.list_width);
        render_tree_list(
            content.list,
            buf,
            " Profiles ",
            "  No profiles found.",
            &state.rows,
            state.selected,
            self.accent,
            &mut state.list_state,
        );

        let hint = " Enter spawn  ↑↓ navigate  Esc cancel";
        render_footer(layout.footer, buf, hint);

        let preview_inner = render_preview_frame(content.preview, buf, self.accent);
        let Some(kind) = state.selected_item() else {
            let empty_text = if state.rows.is_empty() {
                "  No profiles found."
            } else {
                "  Select a profile (●) to preview."
            };
            render_preview_text(preview_inner, buf, empty_text);
            return;
        };

        let lines = truncate_preview_lines(
            kind_preview_lines(kind, Vec::new()),
            preview_inner.height as usize,
        );
        Widget::render(Paragraph::new(lines), preview_inner, buf);
    }
}
