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

use agentik_core::storage::AgentRecord;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::Popup;

// ═══════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════

/// One selectable agent record entry.
#[derive(Clone)]
pub struct AgentRecordItem {
    pub id: uuid::Uuid,
    pub name: String,
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
    pub fn filtered_len(&self) -> usize {
        self.rows.len()
    }

    // ── Data ──

    /// Populate the picker from agent records.
    pub fn set_records(&mut self, records: &[AgentRecord]) {
        self.items = records
            .iter()
            .map(|r| AgentRecordItem {
                id: r.id,
                name: r.name.clone(),
                last_active: r.last_active,
                config_json: r.config_json.clone(),
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
                    .filter(|(_, item)| item.name.to_lowercase().contains(&needle))
                    .map(|(i, _)| i)
                    .collect(),
            )
        };

        // Collect all paths (including intermediate folders) from agent names.
        // Each agent name is a full AgentPath like `/root/researcher/worker`.
        let agent_paths: Vec<(usize, Vec<String>)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| matching.as_ref().map(|m| m.contains(i)).unwrap_or(true))
            .map(|(i, item)| {
                let segments: Vec<String> = item
                    .name
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                (i, segments)
            })
            .collect();

        // When filtering, force-expand all folders that lead to matches.
        let mut force_expand: HashSet<String> = HashSet::new();
        if let Some(ref matches) = matching {
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
            let _ = matches; // suppress unused warning
        }

        // Build the tree. We use a simple recursive approach: collect all
        // unique prefixes, then emit them in sorted order.
        let mut rows = Vec::new();

        // Special case: if there's only one top-level group and it's `root`,
        // we still show it as a collapsible node.
        // Group agent paths by their parent chain and emit in order.
        build_tree(&agent_paths, &self.collapsed, &force_expand, &mut rows, 0);

        self.rows = rows;
        // Clamp selection.
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        // If we had a selection and the list didn't change much, try to
        // keep it stable. Otherwise, reset to first leaf.
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
        if depth + 1 <= segs.len() {
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
            accent: Color::Green,
            popup_width: 0,
            list_width: 32,
        }
    }

    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
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

        let popup = Popup::new(" Resume Agent ")
            .accent(self.accent)
            .width(self.popup_width);
        let inner = popup.render(area, buf);

        // Top-level vertical: search row (1) + separator (1) + content (rest) + footer (1).
        let v_regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1), // separator line
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

        // ── Search input row ──
        let input_line = if state.query.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " search agents…",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(self.accent)),
                Span::styled(state.query.clone(), Style::default().fg(Color::White)),
            ])
        };
        Widget::render(Paragraph::new(input_line), v_regions[0], buf);

        Widget::render(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
            v_regions[1],
            buf,
        );

        // ── Content area: two horizontal blocks ──
        let h_regions = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(self.list_width), Constraint::Min(10)])
            .split(v_regions[2]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_preview_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = if state.delete_confirm_id.is_some() {
            let item_name = state.selected_item().map(|i| i.name).unwrap_or_default();
            format!(
                " Type 'yes' to delete '{}'  Enter confirm  Esc cancel",
                item_name
            )
        } else {
            " Enter resume  →/← expand/fold  Ctrl+D delete  ↑↓ navigate  Esc cancel".to_string()
        };
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[3], buf);

        // ── Delete confirmation overlay ──
        if state.delete_confirm_id.is_some() {
            let confirm_area = Rect {
                x: h_regions[1].x,
                y: h_regions[1].y + h_regions[1].height.saturating_sub(2),
                width: h_regions[1].width,
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
    }
}

impl AgentPicker {
    /// Render the left block: tree of agents.
    fn render_list_block(&self, area: Rect, buf: &mut Buffer, state: &mut AgentPickerState) {
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Agents ",
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if state.rows.is_empty() {
            let line = Line::from(Span::styled(
                "  No agents found.",
                Style::default().fg(Color::DarkGray),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        }

        let items: Vec<ListItem> = state
            .rows
            .iter()
            .enumerate()
            .map(|(sel_i, node)| {
                let is_selected = sel_i == state.selected;
                let indent = "  ".repeat(node.depth);

                if node.is_leaf {
                    // Leaf agent — show with a colored bullet.
                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(self.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    Line::from(vec![
                        Span::styled(indent, Style::default()),
                        Span::styled("● ", Style::default().fg(self.accent)),
                        Span::styled(node.name.clone(), style),
                    ])
                } else {
                    // Folder — show expand/collapse arrow.
                    let arrow = if node.expanded { "▼" } else { "▶" };
                    let folder_style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(self.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    };
                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(self.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::DIM)
                    };
                    Line::from(vec![
                        Span::styled(indent, Style::default()),
                        Span::styled(format!("{arrow} "), folder_style),
                        Span::styled(node.name.clone(), name_style),
                    ])
                }
            })
            .map(ListItem::new)
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(self.accent)
                .add_modifier(Modifier::BOLD),
        );
        StatefulWidget::render(list, inner, buf, &mut state.list_state);
    }

    /// Render the right block: preview of the currently selected agent.
    fn render_preview_block(&self, area: Rect, buf: &mut Buffer, state: &AgentPickerState) {
        let block = Block::default().borders(Borders::NONE).title(Span::styled(
            " Preview ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(item) = state.selected_item() else {
            // Show folder info or "no agent selected".
            let node = state.rows.get(state.selected);
            let msg = if let Some(n) = node {
                if n.is_leaf {
                    "  No agent selected.".to_string()
                } else {
                    format!("  Folder: {}\n  Path: {}", n.name, n.path)
                }
            } else {
                "  No agent selected.".to_string()
            };
            let line = Line::from(Span::styled(
                msg,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        };

        let mut lines: Vec<Line> = Vec::new();

        // Name (full path).
        lines.push(Line::from(vec![
            Span::styled("  Name      ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                item.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // ID.
        lines.push(Line::from(vec![
            Span::styled("  ID        ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", item.id), Style::default().fg(Color::Gray)),
        ]));

        // Last active.
        let last_active_str = chrono::DateTime::from_timestamp_millis(item.last_active)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(Line::from(vec![
            Span::styled("  Last      ", Style::default().fg(Color::DarkGray)),
            Span::styled(last_active_str, Style::default().fg(Color::Gray)),
        ]));

        // Spacer.
        lines.push(Line::from(""));

        // Config (pretty-printed JSON, capped by available height).
        lines.push(Line::from(Span::styled(
            "  Config:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )));

        if let Some(obj) = item.config_json.as_object() {
            for (key, val) in obj {
                let val_str = match val {
                    serde_json::Value::String(s) => format!("\"{}\"", s),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    other => other.to_string(),
                };
                let line = Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(key.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(val_str, Style::default().fg(Color::Gray)),
                ]);
                lines.push(line);
            }
        } else {
            lines.push(Line::from(Span::styled(
                "    (non-object config)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        // Truncate to fit available height.
        let max_lines = inner.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines.saturating_sub(1));
            lines.push(Line::from(Span::styled(
                "    …",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);
    }
}
