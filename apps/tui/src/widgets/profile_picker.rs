//! Profile picker — tree-view widget for selecting an agent profile.
//!
//! Two-column layout: a collapsible tree on the left (organized by profile
//! path hierarchy), and a preview pane on the right showing the selected
//! profile's configuration details. Self-contained (state + rendering + key
//! handling all live here).

use std::collections::HashSet;

use agentik_core::AgentProfile;
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::Color,
    widgets::{ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::tree_picker::{
    PickerTreeRow, profile_preview_lines, render_chrome, render_footer, render_preview_frame,
    render_preview_text, render_search, render_separator, render_tree_list, split_content,
    truncate_preview_lines,
};

/// One selectable profile entry, carrying the full [`AgentProfile`].
#[derive(Clone)]
pub struct ProfileItem {
    pub profile: AgentProfile,
}

/// One visible row in the tree — either a collapsible folder or a leaf profile.
#[derive(Clone, Debug)]
struct TreeNode {
    /// Full path of this node (e.g. `researcher/genomics`).
    path: String,
    /// Short name — last path segment.
    name: String,
    /// Depth in the tree (0 = root).
    depth: usize,
    /// `true` if this is a leaf (an actual profile).
    is_leaf: bool,
    /// Profile item index, if this is a leaf.
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

/// State for the profile picker.
#[derive(Default)]
pub struct ProfilePickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    /// All profiles.
    items: Vec<ProfileItem>,
    /// Flat list of tree rows in display order (only visible / non-collapsed
    /// nodes). Rebuilt whenever the tree or collapse state changes.
    rows: Vec<TreeNode>,
    /// Set of folder paths that are collapsed by the user.
    collapsed: HashSet<String>,
    selected: usize,
    list_state: ListState,
}

impl ProfilePickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.collapsed.clear();
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

    /// Returns a clone of the currently selected profile item, if the cursor
    /// is on a leaf row.
    pub fn selected_item(&self) -> Option<ProfileItem> {
        let node = self.rows.get(self.selected)?;
        let idx = node.item_idx?;
        self.items.get(idx).cloned()
    }

    /// Number of rows currently displayed.

    /// Populate the picker from profiles.
    pub fn set_profiles(&mut self, profiles: Vec<AgentProfile>) {
        self.items = profiles
            .into_iter()
            .map(|p| ProfileItem { profile: p })
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
    /// - Search filter (matching profiles are shown, ancestors auto-expanded)
    fn rebuild_rows(&mut self) {
        let needle = self.query.trim().to_lowercase();

        // Determine which profile indices match the search filter.
        let matching: Option<HashSet<usize>> = if needle.is_empty() {
            None
        } else {
            Some(
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        item.profile.path.to_lowercase().contains(&needle)
                            || item.profile.description.to_lowercase().contains(&needle)
                    })
                    .map(|(i, _)| i)
                    .collect(),
            )
        };

        // Collect profile paths as segment vectors. Profile paths are like
        // "researcher/genomics/mr_analysis" (no /root prefix).
        let profile_paths: Vec<(usize, Vec<String>)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| matching.as_ref().map(|m| m.contains(i)).unwrap_or(true))
            .map(|(i, item)| {
                let segments: Vec<String> =
                    item.profile.path.split('/').map(String::from).collect();
                (i, segments)
            })
            .collect();

        // When filtering, force-expand all folders that lead to matches.
        let mut force_expand: HashSet<String> = HashSet::new();
        if matching.is_some() {
            for (_, segs) in &profile_paths {
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

        let mut rows = Vec::new();
        build_tree(&profile_paths, &self.collapsed, &force_expand, &mut rows, 0);

        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.sync_list_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tree builder
// ═══════════════════════════════════════════════════════════════════════

/// Recursively build the visible `rows` list from profile paths.
fn build_tree(
    entries: &[(usize, Vec<String>)],
    collapsed: &HashSet<String>,
    force_expand: &HashSet<String>,
    rows: &mut Vec<TreeNode>,
    depth: usize,
) {
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

        let pos = seen.iter().position(|(s, _, _)| s == &segment);
        let idx = match pos {
            Some(i) => i,
            None => {
                seen.push((segment.clone(), full_path.clone(), Vec::new()));
                seen.len() - 1
            }
        };

        if depth < segs.len() {
            seen[idx].2.push((item_idx, segs.clone()));
        }
    }

    for (segment, full_path, children) in &seen {
        let leaf_entry = children.iter().find(|(_, segs)| segs.len() == depth + 1);
        let is_leaf = leaf_entry.is_some();
        let item_idx = leaf_entry.map(|(idx, _)| *idx);

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

        if has_children && expanded {
            build_tree(&deeper_children, collapsed, force_expand, rows, depth + 1);
        }
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

        let hint = " Enter spawn  →/← expand/fold  ↑↓ navigate  Esc cancel";
        render_footer(layout.footer, buf, hint);

        let preview_inner = render_preview_frame(content.preview, buf, self.accent);
        let Some(item) = state.selected_item() else {
            let empty_text = if state.rows.is_empty() {
                "  No profiles found."
            } else {
                "  Select a profile (●) to preview."
            };
            render_preview_text(preview_inner, buf, empty_text);
            return;
        };

        let lines = truncate_preview_lines(
            profile_preview_lines(&item.profile, Vec::new()),
            preview_inner.height as usize,
        );
        Widget::render(Paragraph::new(lines), preview_inner, buf);
    }
}
