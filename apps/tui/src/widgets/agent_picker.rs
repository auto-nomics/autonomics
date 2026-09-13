//! Agent resume picker — standalone widget for listing and restoring
//! previously registered agents with their conversation history.
//!
//! Agents are displayed as a **collapsible tree** based on their
//! [`AgentPath`](agentik_types::AgentPath) hierarchical paths:
//!
//! ```text
//! ▼ root
//!   ├─ ● researcher        (agent leaf)
//!   ├─ ▼ analyst            (expanded folder)
//!   │   └─ ● worker         (agent leaf)
//!   └─ ● writer             (agent leaf)
//! ```
//!
//! Built on top of the generic [`Popup`](super::popup::Popup) container.

use std::collections::HashSet;

use agentik_core::{AgentProfile, storage::AgentRecord};
use agentik_types::AgentPath;
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::tree_picker::{
    PickerTreeRow, field_line, profile_preview_lines, render_chrome, render_footer,
    render_preview_frame, render_preview_text, render_search, render_separator, render_tree_list,
    section_line, split_content, truncate_preview_lines,
};

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// One selectable agent record entry.
#[derive(Clone)]
pub struct AgentRecordItem {
    pub id: uuid::Uuid,
    /// Full hierarchical agent path (e.g. `/root/researcher`).
    pub path: AgentPath,
    pub last_active: i64,
    /// Serialized `AgentProfile` stored at registration time. Used to
    /// reconstruct the profile if the named profile no longer exists.
    pub config_json: serde_json::Value,
}

/// One visible row in the tree — either a collapsible folder or a leaf agent.
#[derive(Clone, Debug)]
struct TreeNode {
    /// Full path of this node (e.g. `/root/researcher`).
    path: String,
    /// Short name — last path segment.
    name: String,
    /// Depth in the tree (0 = root).
    depth: usize,
    /// `true` if this is a leaf (an actual agent record).
    is_leaf: bool,
    /// Agent record index, if this is a leaf.
    item_idx: Option<usize>,
    /// `true` if this folder is currently expanded.
    expanded: bool,
}

impl PickerTreeRow for TreeNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn depth(&self) -> usize {
        self.depth
    }

    fn is_leaf(&self) -> bool {
        self.is_leaf
    }

    fn expanded(&self) -> bool {
        self.expanded
    }
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the agent resume picker.
#[derive(Default)]
pub struct AgentPickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    /// All registered agents (loaded from storage).
    items: Vec<AgentRecordItem>,
    /// Flat list of tree rows in display order (only visible / non-collapsed
    /// nodes).  Rebuilt whenever the tree or collapse state changes.
    rows: Vec<TreeNode>,
    /// Set of folder paths that are collapsed by the user.
    collapsed: HashSet<String>,
    selected: usize,
    list_state: ListState,
    /// When `Some`, a delete confirmation is in progress for this agent ID.
    /// The user must type "yes" and press Enter to confirm.
    pub delete_confirm_id: Option<uuid::Uuid>,
    /// Text typed during delete confirmation.
    pub delete_confirm_input: String,
    /// When `Some`, rename mode is active for this agent ID.
    /// The user types a new short name and presses Enter to confirm.
    pub rename_id: Option<uuid::Uuid>,
    /// Text typed during rename.
    pub rename_input: String,
    /// Validation error message for the rename input (shown inline).
    pub rename_error: Option<String>,
}

impl AgentPickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.collapsed.clear();
        self.selected = 0;
        self.rebuild_rows();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.delete_confirm_id = None;
        self.delete_confirm_input.clear();
        self.rename_id = None;
        self.rename_input.clear();
        self.rename_error = None;
    }

    // ── Delete confirmation ──

    pub fn start_delete_confirm(&mut self) {
        if let Some(item) = self.selected_item() {
            self.delete_confirm_id = Some(item.id);
            self.delete_confirm_input.clear();
        }
    }

    pub fn cancel_delete(&mut self) {
        self.delete_confirm_id = None;
        self.delete_confirm_input.clear();
    }

    pub fn check_delete_confirm(&mut self) -> Option<uuid::Uuid> {
        if self.delete_confirm_input.trim().eq_ignore_ascii_case("yes") {
            let id = self.delete_confirm_id.take();
            self.delete_confirm_input.clear();
            id
        } else {
            None
        }
    }

    pub fn remove_by_id(&mut self, id: uuid::Uuid) {
        self.items.retain(|i| i.id != id);
        self.rebuild_rows();
    }

    // ── Rename ──

    /// Enter rename mode for the currently selected leaf agent.
    /// Pre-fills the input with the agent's current short name.
    pub fn start_rename(&mut self) {
        if let Some(item) = self.selected_item() {
            self.rename_id = Some(item.id);
            self.rename_input = item.path.name().to_string();
            self.rename_error = None;
        }
    }

    pub fn cancel_rename(&mut self) {
        self.rename_id = None;
        self.rename_input.clear();
        self.rename_error = None;
    }

    /// Validate the rename input and, if valid, return `(agent_id, new_path)`.
    ///
    /// The new path is constructed by replacing the last segment of the
    /// agent's current path with the user-supplied name. The name must
    /// be a valid `AgentPath` segment (lowercase `[a-z0-9_]`, 1–32 chars).
    pub fn check_rename(&mut self) -> Option<(uuid::Uuid, AgentPath)> {
        let id = self.rename_id?;
        let new_name = self.rename_input.trim();

        // Validate segment.
        if let Err(e) = agentik_types::validate_segment(new_name) {
            self.rename_error = Some(e.to_string());
            return None;
        }
        self.rename_error = None;

        // Find the agent's current path and replace the last segment.
        let item = self.items.iter().find(|i| i.id == id)?;
        let parent = item.path.parent();
        let new_path = match &parent {
            Some(p) => p.join(new_name).unwrap_or_else(|_| {
                // join only fails on invalid segment, already validated above.
                AgentPath::root().join(new_name).unwrap()
            }),
            None => AgentPath::root(), // root can't be renamed
        };

        // Clear rename state (commit).
        self.rename_id = None;
        self.rename_input.clear();

        Some((id, new_path))
    }

    /// Update an agent's path in the in-memory item list (after storage
    /// confirms the rename succeeded).
    pub fn apply_rename(&mut self, id: uuid::Uuid, new_path: AgentPath) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.path = new_path;
        }
        self.rebuild_rows();
    }

    // ── Search ──

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.rebuild_rows();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.rebuild_rows();
    }

    // ── Navigation ──

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

    /// Expand or collapse the folder at the current cursor position.
    /// No-op on leaf nodes.
    pub fn toggle_expand(&mut self) {
        if let Some(node) = self.rows.get(self.selected) {
            if !node.is_leaf {
                if self.collapsed.contains(&node.path) {
                    self.collapsed.remove(&node.path);
                } else {
                    self.collapsed.insert(node.path.clone());
                }
                self.rebuild_rows();
            }
        }
    }

    /// Returns a clone of the currently selected agent item, if the cursor
    /// is on a leaf row.
    pub fn selected_item(&self) -> Option<AgentRecordItem> {
        let node = self.rows.get(self.selected)?;
        let idx = node.item_idx?;
        self.items.get(idx).cloned()
    }

    /// Number of rows currently displayed.

    // ── Data ──

    /// Populate the picker from agent records.
    ///
    /// Each record's `name` field is expected to contain the full
    /// hierarchical AgentPath (e.g. `/root/researcher`). Records whose
    /// name cannot be parsed as a valid AgentPath are skipped with a
    /// warning.
    pub fn set_records(&mut self, records: &[AgentRecord]) {
        self.items = records
            .iter()
            .filter_map(|r| match AgentPath::try_from(r.name.as_str()) {
                Ok(path) => Some(AgentRecordItem {
                    id: r.id,
                    path,
                    last_active: r.last_active,
                    config_json: r.config_json.clone(),
                }),
                Err(e) => {
                    tracing::warn!(id = %r.id, name = %r.name, error = %e, "skipping agent record with invalid path");
                    None
                }
            })
            .collect();
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

    /// Rebuild the flat `rows` vector from `items`, honoring:
    /// - Collapse state (collapsed folders hide their children)
    /// - Search filter (matching agents are shown, ancestors auto-expanded)
    fn rebuild_rows(&mut self) {
        let needle = self.query.trim().to_lowercase();

        // Determine which agent indices match the search filter.
        let matching: Option<HashSet<usize>> = if needle.is_empty() {
            None // no filter — all agents visible
        } else {
            Some(
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.path.as_str().to_lowercase().contains(&needle))
                    .map(|(i, _)| i)
                    .collect(),
            )
        };

        // Collect all paths (including intermediate folders) from agent paths.
        // Each item's `path` is a validated AgentPath like `/root/researcher/worker`.
        let agent_paths: Vec<(usize, Vec<String>)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| matching.as_ref().map(|m| m.contains(i)).unwrap_or(true))
            .map(|(i, item)| {
                let segments: Vec<String> =
                    item.path.segments().iter().map(|s| s.to_string()).collect();
                (i, segments)
            })
            .collect();

        // When filtering, force-expand all folders that lead to matches.
        let mut force_expand: HashSet<String> = HashSet::new();
        if matching.is_some() {
            for (_, segs) in &agent_paths {
                let mut acc = String::new();
                for seg in segs {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(seg);
                    force_expand.insert(acc.clone());
                }
            }
        }

        // Build the tree recursively.
        let mut rows = Vec::new();
        build_tree(&agent_paths, &self.collapsed, &force_expand, &mut rows, 0);

        self.rows = rows;
        // Clamp selection.
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.sync_list_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tree builder
// ═══════════════════════════════════════════════════════════════════════

/// Recursively build the visible `rows` list from agent paths.
///
/// - `entries`: `(item_idx, segments)` for each agent.
/// - `collapsed`: user-collapsed folder paths.
/// - `force_expand`: paths that must be expanded (search-match ancestors).
fn build_tree(
    entries: &[(usize, Vec<String>)],
    collapsed: &HashSet<String>,
    force_expand: &HashSet<String>,
    rows: &mut Vec<TreeNode>,
    depth: usize,
) {
    // Collect unique child segments at this depth.
    // Each entry: (segment, full_path, child_entries)
    // A child can be either a leaf (the full path matches an agent) or
    // an intermediate folder.
    let mut seen: Vec<(String, String, Vec<(usize, Vec<String>)>)> = Vec::new();

    for &(item_idx, ref segs) in entries {
        if depth >= segs.len() {
            continue;
        }
        let segment = segs[depth].clone();
        let mut full_path = String::new();
        for s in &segs[..=depth] {
            if !full_path.is_empty() {
                full_path.push('/');
            }
            full_path.push_str(s);
        }

        // Find or create the group.
        let pos = seen.iter().position(|(s, _, _)| s == &segment);
        let idx = match pos {
            Some(i) => i,
            None => {
                seen.push((segment.clone(), full_path.clone(), Vec::new()));
                seen.len() - 1
            }
        };

        // Pass remaining segments to children.
        if depth < segs.len() {
            seen[idx].2.push((item_idx, segs.clone()));
        }
    }

    for (segment, full_path, children) in &seen {
        // Is this node a leaf? It's a leaf if any entry's segments end here.
        let leaf_entry = children.iter().find(|(_, segs)| segs.len() == depth + 1);

        let is_leaf = leaf_entry.is_some();
        let item_idx = leaf_entry.map(|(idx, _)| *idx);

        // Intermediate children (depth+1 onward).
        let deeper_children: Vec<(usize, Vec<String>)> = children
            .iter()
            .filter(|(_, segs)| segs.len() > depth + 1)
            .cloned()
            .collect();

        let has_children = !deeper_children.is_empty();
        let is_collapsed = collapsed.contains(full_path) && !force_expand.contains(full_path);
        let expanded = !is_collapsed;

        rows.push(TreeNode {
            path: full_path.clone(),
            name: segment.clone(),
            depth,
            is_leaf,
            item_idx,
            expanded: has_children && expanded,
        });

        // Recurse into children if expanded.
        if has_children && expanded {
            build_tree(&deeper_children, collapsed, force_expand, rows, depth + 1);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// Standalone widget that renders the agent resume picker as a centered popup
/// with two blocks: a tree agent list (left) and a preview pane (right).
pub struct AgentPicker {
    pub accent: Color,
    /// Width of the popup (0 = auto, ~80% of frame).
    pub popup_width: u16,
    /// Width of the left list block.
    pub list_width: u16,
}

impl AgentPicker {
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

impl Default for AgentPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for AgentPicker {
    type State = AgentPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut AgentPickerState) {
        if !state.visible {
            return;
        }

        let layout = render_chrome(area, buf, " Resume Agent ", self.accent, self.popup_width);
        render_search(layout.search, buf, self.accent, &state.query, "agents");
        render_separator(layout.separator, buf);

        let content = split_content(layout.content, self.list_width);
        render_tree_list(
            content.list,
            buf,
            " Agents ",
            "  No agents found.",
            &state.rows,
            state.selected,
            self.accent,
            &mut state.list_state,
        );

        let preview_inner = render_preview_frame(content.preview, buf, self.accent);
        if let Some(item) = state.selected_item() {
            let last_active = chrono::DateTime::from_timestamp_millis(item.last_active)
                .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let leading_fields = vec![
                ("Path", item.path.as_str().to_string()),
                ("ID", item.id.to_string()),
                ("Last", last_active),
            ];

            let lines = if let Ok(profile) =
                serde_json::from_value::<AgentProfile>(item.config_json.clone())
            {
                profile_preview_lines(&profile, leading_fields)
            } else {
                let mut lines = leading_fields
                    .into_iter()
                    .map(|(label, value)| field_line(label, value))
                    .collect::<Vec<_>>();
                lines.push(Line::from(""));
                lines.push(section_line("Config"));
                match &item.config_json {
                    serde_json::Value::Object(config) => {
                        for (key, value) in config {
                            let value = match value {
                                serde_json::Value::String(value) => value.clone(),
                                serde_json::Value::Bool(value) => value.to_string(),
                                serde_json::Value::Number(value) => value.to_string(),
                                serde_json::Value::Null => "null".to_string(),
                                other => other.to_string(),
                            };
                            lines.push(field_line(key, value));
                        }
                    }
                    other => lines.push(field_line("Value", other.to_string())),
                }
                lines
            };

            Widget::render(
                Paragraph::new(truncate_preview_lines(lines, preview_inner.height as usize)),
                preview_inner,
                buf,
            );
        } else {
            let empty_text = if state.rows.is_empty() {
                "  No agents found."
            } else {
                "  Select an agent (●) to preview."
            };
            render_preview_text(preview_inner, buf, empty_text);
        }

        // ── Footer ──
        let hint = if state.rename_id.is_some() {
            " Enter confirm rename  Esc cancel".to_string()
        } else if state.delete_confirm_id.is_some() {
            let item_name = state
                .selected_item()
                .map(|i| i.path.as_str().to_string())
                .unwrap_or_default();
            format!(
                " Type 'yes' to delete '{}'  Enter confirm  Esc cancel",
                item_name
            )
        } else {
            " Enter resume  →/← expand/fold  Ctrl+R rename  Ctrl+D delete  ↑↓ navigate  Esc cancel"
                .to_string()
        };
        render_footer(layout.footer, buf, &hint);

        // ── Delete confirmation overlay ──
        if state.delete_confirm_id.is_some() {
            let confirm_area = Rect {
                x: content.preview.x,
                y: content.preview.y + content.preview.height.saturating_sub(2),
                width: content.preview.width,
                height: 2,
            };
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Red));
            let inner_confirm = block.inner(confirm_area);
            block.render(confirm_area, buf);

            let line = Line::from(vec![
                Span::styled("  Confirm delete? ", Style::default().fg(Color::Red)),
                Span::styled(
                    state.delete_confirm_input.clone(),
                    Style::default().fg(Color::White),
                ),
                Span::styled("▏", Style::default().fg(Color::Red)),
            ]);
            Widget::render(Paragraph::new(line), inner_confirm, buf);
        }

        // ── Rename overlay ──
        if state.rename_id.is_some() {
            let rename_area = Rect {
                x: content.preview.x,
                y: content.preview.y + content.preview.height.saturating_sub(2),
                width: content.preview.width,
                height: 2,
            };
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Cyan));
            let inner_rename = block.inner(rename_area);
            block.render(rename_area, buf);

            let error_or_cursor = if let Some(ref err) = state.rename_error {
                Line::from(vec![
                    Span::styled("  ✗ ", Style::default().fg(Color::Red)),
                    Span::styled(err.clone(), Style::default().fg(Color::Red)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("  New name: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        state.rename_input.clone(),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        "▏",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::SLOW_BLINK),
                    ),
                ])
            };
            Widget::render(Paragraph::new(error_or_cursor), inner_rename, buf);
        }
    }
}
