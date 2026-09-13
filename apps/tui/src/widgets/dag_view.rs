//! Interactive read-only DAG canvas for the data-engine graph.
//!
//! The widget deliberately consumes an owned [`DagTuiSnapshot`] instead of the
//! live engine. Rendering is layered top-down: topological levels become rows,
//! crossing reduction orders each row, and edges are routed orthogonally.

use std::collections::HashMap;

use dag_core::dag::{DagTuiSnapshot, RuntimeStatus};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;

const NODE_WIDTH: u16 = 18;
const NODE_HEIGHT: u16 = 3;
const NODE_GAP: u16 = 5;
const LEVEL_GAP: u16 = 3;
const CHANNEL_GAP: u16 = 3;

/// Selection and viewport state, kept separate from the graph snapshot.
#[derive(Debug, Clone, Default)]
pub struct DagViewState {
    selected: Option<String>,
    scroll: (u16, u16),
}

impl DagViewState {
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, id: impl Into<String>) {
        self.selected = Some(id.into());
    }

    pub fn clear(&mut self) {
        self.selected = None;
        self.scroll = (0, 0);
    }

    /// Select the first predecessor, preferring the closest column.
    pub fn select_up(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_related(snapshot, Direction::Up);
    }

    pub fn select_down(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_related(snapshot, Direction::Down);
    }

    pub fn select_left(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_sibling(snapshot, -1);
    }

    pub fn select_right(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_sibling(snapshot, 1);
    }

    pub fn select_next(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_offset(snapshot, 1);
    }

    pub fn select_previous(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_offset(snapshot, snapshot.nodes.len().saturating_sub(1));
    }

    fn select_offset(&mut self, snapshot: &DagTuiSnapshot, offset: usize) {
        if snapshot.nodes.is_empty() {
            self.selected = None;
            return;
        }
        if self.selected.is_none() {
            let initial = if offset + 1 == snapshot.nodes.len() {
                snapshot.nodes.len() - 1
            } else {
                0
            };
            self.selected = Some(snapshot.nodes[initial].id.clone());
            return;
        }
        let current = self
            .selected
            .as_deref()
            .and_then(|id| snapshot.nodes.iter().position(|node| node.id == id))
            .unwrap_or(0);
        let next = (current + offset) % snapshot.nodes.len();
        self.selected = Some(snapshot.nodes[next].id.clone());
    }

    fn select_sibling(&mut self, snapshot: &DagTuiSnapshot, delta: isize) {
        let Some(current) = self.current_index(snapshot) else {
            return;
        };
        let level = self.level_of(snapshot, current);
        let same_level: Vec<usize> = snapshot
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, _)| self.level_of(snapshot, *index) == level)
            .map(|(index, _)| index)
            .collect();
        let Some(pos) = same_level.iter().position(|index| *index == current) else {
            return;
        };
        let next_pos = (pos as isize + delta).rem_euclid(same_level.len() as isize) as usize;
        self.selected = Some(snapshot.nodes[same_level[next_pos]].id.clone());
    }

    fn select_related(&mut self, snapshot: &DagTuiSnapshot, direction: Direction) {
        let Some(current) = self.current_index(snapshot) else {
            return;
        };
        let node = &snapshot.nodes[current];
        let related: Vec<&str> = match direction {
            Direction::Up => snapshot
                .edges
                .iter()
                .filter(|edge| edge.to == node.id)
                .filter_map(|edge| snapshot.node(&edge.from).map(|node| node.id.as_str()))
                .collect(),
            Direction::Down => snapshot
                .edges
                .iter()
                .filter(|edge| edge.from == node.id)
                .filter_map(|edge| snapshot.node(&edge.to).map(|node| node.id.as_str()))
                .collect(),
        };
        if related.is_empty() {
            return;
        }
        // Snapshot nodes are id-sorted, so this is deterministic when several
        // edges fan in or out from the selected node.
        let next = if direction == Direction::Up {
            related[0]
        } else {
            related[related.len() - 1]
        };
        self.selected = Some(next.to_string());
    }

    fn current_index(&self, snapshot: &DagTuiSnapshot) -> Option<usize> {
        let selected = self
            .selected
            .as_deref()
            .or_else(|| snapshot.nodes.first().map(|node| node.id.as_str()))?;
        snapshot.nodes.iter().position(|node| node.id == selected)
    }

    fn level_of(&self, snapshot: &DagTuiSnapshot, node_index: usize) -> usize {
        dag_levels(snapshot)
            .get(node_index)
            .copied()
            .unwrap_or_default()
    }
}

/// A reusable, immutable snapshot renderer.
///
/// Render through [`StatefulWidget`] so navigation state can update during the
/// frame (initial selection and viewport fitting).
#[derive(Debug, Clone, Copy)]
pub struct DagTuiWidget<'a> {
    pub snapshot: &'a DagTuiSnapshot,
    pub focused: bool,
}

impl<'a> DagTuiWidget<'a> {
    pub fn new(snapshot: &'a DagTuiSnapshot) -> Self {
        Self {
            snapshot,
            focused: false,
        }
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl StatefulWidget for DagTuiWidget<'_> {
    type State = DagViewState;

    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer, state: &mut Self::State) {
        let accent = if self.focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent))
            .title(Line::from(vec![
                Span::styled(
                    " DAG ",
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "{} nodes · {} edges · {} running · {} failed ",
                        self.snapshot.nodes.len(),
                        self.snapshot.edges.len(),
                        self.snapshot.status_count(RuntimeStatus::Running),
                        self.snapshot.status_count(RuntimeStatus::Failed),
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        let canvas = block.inner(area);
        block.render(area, buf);

        if canvas.width == 0 || canvas.height == 0 {
            return;
        }
        if self.snapshot.nodes.is_empty() {
            buf.set_string(
                canvas.left(),
                canvas.top(),
                "No DAG nodes. Ask the agent to add one.",
                Style::default().fg(Color::DarkGray),
            );
            return;
        }

        if state.selected.is_none() {
            state.selected = self.snapshot.nodes.first().map(|node| node.id.clone());
        }
        let layout = dag_layout(self.snapshot);
        fit_viewport(state, canvas, &layout);

        for edge in &self.snapshot.edges {
            render_edge(buf, canvas, state, &layout, self.snapshot, edge);
        }

        let selected_id = state.selected.clone();
        for node in &self.snapshot.nodes {
            let Some(virtual_node) = layout.nodes.get(&node.id) else {
                continue;
            };
            render_node(
                buf,
                canvas,
                state,
                virtual_node,
                node,
                selected_id.as_deref() == Some(node.id.as_str()),
            );
        }

        render_status_line(buf, area, self.snapshot, state, self.focused);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy)]
struct VirtualNode {
    x: u16,
    y: u16,
    level: usize,
}

impl VirtualNode {
    fn center_x(&self) -> u16 {
        self.x + NODE_WIDTH / 2
    }

    fn bottom(&self) -> u16 {
        self.y + NODE_HEIGHT - 1
    }
}

#[derive(Debug, Default)]
struct DagLayout {
    nodes: HashMap<String, VirtualNode>,
    virtual_size: (u16, u16),
}

/// Longest-path levels. DAG edges are acyclic by construction; disconnected
/// nodes become level zero.
fn dag_levels(snapshot: &DagTuiSnapshot) -> Vec<usize> {
    let mut levels = vec![0usize; snapshot.nodes.len()];
    let mut position: HashMap<&str, usize> = HashMap::with_capacity(snapshot.nodes.len());
    for (index, node) in snapshot.nodes.iter().enumerate() {
        position.insert(node.id.as_str(), index);
    }

    for _ in 0..snapshot.nodes.len() {
        let mut changed = false;
        for edge in &snapshot.edges {
            let (Some(from), Some(to)) = (
                position.get(edge.from.as_str()),
                position.get(edge.to.as_str()),
            ) else {
                continue;
            };
            let next = levels[*from] + 1;
            if next > levels[*to] {
                levels[*to] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    levels
}

fn dag_layout(snapshot: &DagTuiSnapshot) -> DagLayout {
    let levels = dag_levels(snapshot);
    let max_level = levels.iter().copied().max().unwrap_or_default();
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); max_level + 1];
    // Snapshot is id-sorted, giving the deterministic starting order.
    for (index, level) in levels.iter().enumerate() {
        groups[*level].push(index);
    }

    // A compact barycenter sweep removes the worst fan-in/fan-out crossings.
    let mut order: HashMap<&str, usize> = HashMap::with_capacity(snapshot.nodes.len());
    for sweep in 0..4 {
        let top_down = sweep % 2 == 0;
        let levels_to_process = if top_down {
            (1..groups.len()).collect::<Vec<_>>()
        } else {
            (0..groups.len().saturating_sub(1))
                .rev()
                .collect::<Vec<_>>()
        };
        for level in levels_to_process {
            let neighbor_level = if top_down { level - 1 } else { level + 1 };
            let neighbor_len = groups[neighbor_level].len();
            order.clear();
            for (order_index, node_index) in groups[neighbor_level].iter().enumerate() {
                order.insert(snapshot.nodes[*node_index].id.as_str(), order_index);
            }
            groups[level].sort_unstable_by_key(|node_index| {
                let id = &snapshot.nodes[*node_index].id;
                let values = snapshot
                    .edges
                    .iter()
                    .filter_map(|edge| {
                        let relevant = if top_down {
                            edge.to == *id
                        } else {
                            edge.from == *id
                        };
                        if relevant {
                            order
                                .get(edge.from.as_str())
                                .or_else(|| order.get(edge.to.as_str()))
                                .copied()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<usize>>();
                average_or_center(&values, neighbor_len)
            });
        }
    }

    let mut layout = DagLayout::default();
    let mut adjacent_channel_count = 0usize;
    for (level, group) in groups.iter().enumerate() {
        for (column, node_index) in group.iter().enumerate() {
            let node = &snapshot.nodes[*node_index];
            layout.nodes.insert(
                node.id.clone(),
                VirtualNode {
                    x: (column * (NODE_WIDTH + NODE_GAP) as usize) as u16,
                    y: (level * (NODE_HEIGHT + LEVEL_GAP) as usize) as u16,
                    level,
                },
            );
        }
    }

    for edge in &snapshot.edges {
        let (Some(from), Some(to)) = (layout.nodes.get(&edge.from), layout.nodes.get(&edge.to))
        else {
            continue;
        };
        if to.level > from.level + 1 {
            adjacent_channel_count += 1;
        }
    }

    let graph_width = groups
        .iter()
        .map(|group| group.len().saturating_sub(1))
        .max()
        .unwrap_or_default()
        * (NODE_WIDTH + NODE_GAP) as usize
        + NODE_WIDTH as usize;
    let channel_width = if adjacent_channel_count == 0 {
        0
    } else {
        adjacent_channel_count * CHANNEL_GAP as usize + 1
    };
    let height = (max_level * (NODE_HEIGHT + LEVEL_GAP) as usize) + NODE_HEIGHT as usize;
    layout.virtual_size = (
        (graph_width + channel_width).min(u16::MAX as usize) as u16,
        height.min(u16::MAX as usize) as u16,
    );
    layout
}

fn average_or_center(values: &[usize], fallback_len: usize) -> usize {
    if values.is_empty() {
        fallback_len / 2
    } else {
        values.iter().sum::<usize>() / values.len()
    }
}

fn render_node(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    virtual_node: &VirtualNode,
    node: &dag_core::dag::DagNodeView,
    selected: bool,
) {
    let Some(rect) = virtual_rect(canvas, state, virtual_node) else {
        return;
    };
    let (border_color, label_color) = if selected {
        (
            Color::Yellow,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Color::Cyan, Style::default().fg(Color::Gray))
    };
    let (icon, icon_color) = status_marker(node.status);
    let border = Style::default().fg(border_color);

    draw_hline(buf, canvas, rect.x, rect.y, rect.width, border);
    draw_hline(buf, canvas, rect.x, rect.bottom(), rect.width, border);
    draw_vline(buf, canvas, rect.x, rect.y, rect.height, border);
    draw_vline(buf, canvas, rect.right(), rect.y, rect.height, border);
    draw_node_corners(buf, canvas, state, virtual_node, border);

    let title_width = rect.width.saturating_sub(4) as usize;
    let title = format!("{icon} {}", node.id);
    buf.set_string(
        rect.x + 1,
        rect.y + 1,
        truncate(&title, title_width),
        Style::default().fg(icon_color),
    );
    let kind_width = rect.width.saturating_sub(4) as usize;
    let kind = format!("  {}", node.kind);
    buf.set_string(
        rect.x + 1,
        rect.y + 2,
        truncate(&kind, kind_width),
        label_color,
    );

    // Port indicators make fan-in/fan-out visible even when an edge is clipped.
    if !node.inputs.is_empty() {
        set_virtual(
            buf,
            canvas,
            state,
            virtual_node.x,
            virtual_node.y + 1,
            "│",
            border,
        );
    }
    if !node.outputs.is_empty() {
        set_virtual(
            buf,
            canvas,
            state,
            virtual_node.x + NODE_WIDTH - 1,
            virtual_node.y + 1,
            "│",
            border,
        );
    }
    if node.dirty {
        set_virtual(
            buf,
            canvas,
            state,
            virtual_node.x + NODE_WIDTH - 2,
            virtual_node.y,
            "*",
            Style::default().fg(Color::Yellow),
        );
    }
}

fn render_edge(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    layout: &DagLayout,
    snapshot: &DagTuiSnapshot,
    edge: &dag_core::dag::DagEdgeView,
) {
    let (Some(from), Some(to)) = (layout.nodes.get(&edge.from), layout.nodes.get(&edge.to)) else {
        return;
    };
    let selected = state
        .selected
        .as_deref()
        .is_some_and(|id| id == edge.from || id == edge.to);
    let style = if selected {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    if to.level == from.level + 1 {
        let x = from.center_x();
        let target_x = to.center_x();
        let bend_y = from.bottom() + LEVEL_GAP / 2;
        draw_virtual_v(buf, canvas, state, x, from.bottom() + 1, bend_y, style);
        draw_virtual_h(
            buf,
            canvas,
            state,
            bend_y,
            x.min(target_x),
            x.max(target_x),
            style,
        );
        draw_virtual_v(
            buf,
            canvas,
            state,
            target_x,
            bend_y,
            to.y.saturating_sub(1),
            style,
        );
        if target_x > x {
            set_virtual(buf, canvas, state, x, bend_y, "└", style);
            set_virtual(buf, canvas, state, target_x, bend_y, "┐", style);
        } else if target_x < x {
            set_virtual(buf, canvas, state, x, bend_y, "┘", style);
            set_virtual(buf, canvas, state, target_x, bend_y, "┌", style);
        }
        return;
    }

    // Route long edges through a dedicated channel to the right of the graph,
    // so they cannot cross unrelated node boxes in intermediate levels.
    let lane_index = snapshot
        .edges
        .iter()
        .take_while(|candidate| candidate.from != edge.from || candidate.to != edge.to)
        .filter(|candidate| {
            layout
                .nodes
                .get(&candidate.to)
                .zip(layout.nodes.get(&candidate.from))
                .is_some_and(|(to, from)| to.level > from.level + 1)
        })
        .count();
    let channel_x = channel_origin(layout) + 1 + (lane_index as u16).saturating_mul(CHANNEL_GAP);
    let entry_y = from.bottom() + 1;
    let exit_y = to.y.saturating_sub(1);
    draw_virtual_h(
        buf,
        canvas,
        state,
        entry_y,
        from.center_x(),
        channel_x,
        style,
    );
    draw_virtual_v(buf, canvas, state, channel_x, entry_y, exit_y, style);
    draw_virtual_h(buf, canvas, state, exit_y, channel_x, to.center_x(), style);
    set_virtual(buf, canvas, state, from.center_x(), entry_y, "└", style);
    set_virtual(buf, canvas, state, channel_x, entry_y, "┌", style);
    set_virtual(buf, canvas, state, channel_x, exit_y, "┘", style);
}

fn draw_node_corners(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    node: &VirtualNode,
    style: Style,
) {
    let right = node.x + NODE_WIDTH - 1;
    let bottom = node.y + NODE_HEIGHT - 1;
    set_virtual(buf, canvas, state, node.x, node.y, "╭", style);
    set_virtual(buf, canvas, state, right, node.y, "╮", style);
    set_virtual(buf, canvas, state, node.x, bottom, "╰", style);
    set_virtual(buf, canvas, state, right, bottom, "╯", style);
}

fn channel_origin(layout: &DagLayout) -> u16 {
    layout
        .nodes
        .values()
        .map(|node| node.x + NODE_WIDTH)
        .max()
        .unwrap_or_default()
}

fn render_status_line(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    snapshot: &DagTuiSnapshot,
    state: &DagViewState,
    focused: bool,
) {
    let Some(node) = state.selected.as_deref().and_then(|id| snapshot.node(id)) else {
        return;
    };
    let (icon, color) = status_marker(node.status);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(icon, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            node.id.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {}", node.kind),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            if node.dirty {
                " · dirty"
            } else {
                " · cached"
            },
            Style::default().fg(if node.dirty {
                Color::Yellow
            } else {
                Color::Green
            }),
        ),
    ];
    if focused {
        spans.push(Span::styled(
            "  ←→ siblings · ↑↓ dataflow · Tab next · R refresh · Esc close",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let line = Line::from(spans);
    let y = area.bottom().saturating_sub(1);
    let mut x = area.left();
    for span in line.spans {
        let span_width = span.width() as u16;
        if span_width == 0 || x >= area.right() {
            break;
        }
        let clipped_width = span_width.min(area.right() - x);
        let text = truncate(&span.content, clipped_width as usize);
        buf.set_string(x, y, text, span.style);
        x = x.saturating_add(clipped_width);
    }
}

fn status_marker(status: RuntimeStatus) -> (&'static str, Color) {
    match status {
        RuntimeStatus::Pending => ("○", Color::DarkGray),
        RuntimeStatus::Ready => ("◐", Color::Yellow),
        RuntimeStatus::Running => ("◉", Color::Cyan),
        RuntimeStatus::Success => ("●", Color::Green),
        RuntimeStatus::Failed => ("✗", Color::Red),
        RuntimeStatus::Skipped => ("◍", Color::DarkGray),
    }
}

fn fit_viewport(state: &mut DagViewState, canvas: Rect, layout: &DagLayout) {
    let (virtual_width, virtual_height) = layout.virtual_size;
    state.scroll.0 = fit_axis(
        state.scroll.0,
        canvas.width,
        virtual_width,
        state
            .selected
            .as_deref()
            .and_then(|id| layout.nodes.get(id))
            .map(|node| node.center_x()),
    );
    state.scroll.1 = fit_axis(
        state.scroll.1,
        canvas.height,
        virtual_height,
        state
            .selected
            .as_deref()
            .and_then(|id| layout.nodes.get(id))
            .map(|node| node.y + NODE_HEIGHT / 2),
    );
}

fn fit_axis(current: u16, viewport: u16, virtual_size: u16, focus: Option<u16>) -> u16 {
    if viewport == 0 || virtual_size <= viewport {
        return 0;
    }
    let max = virtual_size - viewport;
    let desired = focus
        .map(|position| position.saturating_sub(viewport / 2))
        .unwrap_or(current);
    desired.min(max)
}

fn virtual_rect(canvas: Rect, state: &DagViewState, node: &VirtualNode) -> Option<Rect> {
    let (scroll_x, scroll_y) = state.scroll;
    let x = layout_to_screen(canvas.x, canvas.width, node.x, scroll_x)?;
    let y = layout_to_screen(canvas.y, canvas.height, node.y, scroll_y)?;
    Some(Rect {
        x,
        y,
        width: NODE_WIDTH,
        height: NODE_HEIGHT,
    })
}

fn layout_to_screen(origin: u16, size: u16, value: u16, scroll: u16) -> Option<u16> {
    if value < scroll {
        return None;
    }
    let relative = value - scroll;
    if relative >= size {
        return None;
    }
    Some(origin + relative)
}

fn set_virtual(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    virtual_x: u16,
    virtual_y: u16,
    symbol: &str,
    style: Style,
) {
    let (scroll_x, scroll_y) = state.scroll;
    let Some(x) = layout_to_screen(canvas.x, canvas.width, virtual_x, scroll_x) else {
        return;
    };
    let Some(y) = layout_to_screen(canvas.y, canvas.height, virtual_y, scroll_y) else {
        return;
    };
    buf.set_string(x, y, symbol, style);
}

fn draw_virtual_h(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    virtual_y: u16,
    start_x: u16,
    end_x: u16,
    style: Style,
) {
    if start_x <= end_x {
        for x in start_x..=end_x {
            set_virtual(buf, canvas, state, x, virtual_y, "─", style);
        }
    } else {
        for x in end_x..=start_x {
            set_virtual(buf, canvas, state, x, virtual_y, "─", style);
        }
    }
}

fn draw_virtual_v(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    state: &DagViewState,
    virtual_x: u16,
    start_y: u16,
    end_y: u16,
    style: Style,
) {
    if start_y >= end_y {
        return;
    }
    for y in start_y..=end_y {
        set_virtual(buf, canvas, state, virtual_x, y, "│", style);
    }
}

fn draw_hline(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    x: u16,
    y: u16,
    width: u16,
    style: Style,
) {
    for offset in 0..width {
        let cell_x = x + offset;
        if cell_x < canvas.right() && y < canvas.bottom() {
            buf.set_string(cell_x, y, "─", style);
        }
    }
}

fn draw_vline(
    buf: &mut ratatui::buffer::Buffer,
    canvas: Rect,
    x: u16,
    y: u16,
    height: u16,
    style: Style,
) {
    for offset in 0..height {
        let cell_y = y + offset;
        if x < canvas.right() && cell_y < canvas.bottom() {
            buf.set_string(x, cell_y, "│", style);
        }
    }
}

fn truncate(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut width = 0usize;
    for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(value, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width + grapheme_width > max_cells.saturating_sub(1) && width > 0 {
            result.push('…');
            break;
        }
        if width + grapheme_width > max_cells {
            break;
        }
        width += grapheme_width;
        result.push_str(grapheme);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use dag_core::dag::{DagEdgeView, DagNodeView};
    use ratatui::buffer::Buffer;

    fn snapshot() -> DagTuiSnapshot {
        let node = |id: &str| DagNodeView {
            id: id.to_string(),
            kind: "echo".to_string(),
            status: RuntimeStatus::Pending,
            dirty: true,
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        DagTuiSnapshot {
            nodes: vec![node("a"), node("b"), node("c")],
            edges: vec![
                DagEdgeView {
                    from: "a".to_string(),
                    from_port: 0,
                    to: "b".to_string(),
                    to_port: 0,
                },
                DagEdgeView {
                    from: "b".to_string(),
                    from_port: 0,
                    to: "c".to_string(),
                    to_port: 0,
                },
            ],
        }
    }

    #[test]
    fn layered_layout_orders_adjacent_nodes() {
        let layout = dag_layout(&snapshot());
        assert_eq!(layout.nodes["a"].level, 0);
        assert_eq!(layout.nodes["b"].level, 1);
        assert_eq!(layout.nodes["c"].level, 2);
        assert_eq!(layout.nodes["a"].x, layout.nodes["b"].x);
        assert_eq!(layout.nodes["b"].x, layout.nodes["c"].x);
        assert_eq!(
            layout.nodes["b"].y - layout.nodes["a"].y,
            NODE_HEIGHT + LEVEL_GAP
        );
    }

    #[test]
    fn selection_follows_dataflow_and_wraps() {
        let graph = snapshot();
        let mut state = DagViewState::default();
        state.select_next(&graph);
        assert_eq!(state.selected().unwrap(), "a");
        state.select_down(&graph);
        assert_eq!(state.selected().unwrap(), "b");
        state.select_down(&graph);
        assert_eq!(state.selected().unwrap(), "c");
        state.select_up(&graph);
        assert_eq!(state.selected().unwrap(), "b");
        state.select_previous(&graph);
        assert_eq!(state.selected().unwrap(), "a");
    }

    #[test]
    fn node_border_uses_rounded_corners() {
        let graph = snapshot();
        let node = &graph.nodes[0];
        let virtual_node = VirtualNode {
            x: 4,
            y: 2,
            level: 0,
        };
        let mut state = DagViewState::default();
        state.select(node.id.as_str());
        let area = Rect::new(0, 0, 48, 12);
        let mut buf = Buffer::empty(area);

        render_node(&mut buf, area, &state, &virtual_node, node, true);

        let right = virtual_node.x + NODE_WIDTH - 1;
        let bottom = virtual_node.y + NODE_HEIGHT - 1;
        assert_eq!(buf[(virtual_node.x, virtual_node.y)].symbol(), "╭");
        assert_eq!(buf[(right, virtual_node.y)].symbol(), "╮");
        assert_eq!(buf[(virtual_node.x, bottom)].symbol(), "╰");
        assert_eq!(buf[(right, bottom)].symbol(), "╯");
        assert_eq!(buf[(right - 1, virtual_node.y)].symbol(), "*");
    }

    #[test]
    fn adjacent_edge_uses_turn_corners() {
        let graph = snapshot();
        let mut layout = dag_layout(&graph);
        layout.nodes.get_mut("b").unwrap().x = NODE_WIDTH + NODE_GAP;
        let state = DagViewState::default();
        let area = Rect::new(0, 0, 96, 20);
        let mut buf = Buffer::empty(area);

        render_edge(&mut buf, area, &state, &layout, &graph, &graph.edges[0]);

        let source = layout.nodes["a"].center_x();
        let target = layout.nodes["b"].center_x();
        let bend_y = layout.nodes["a"].bottom() + LEVEL_GAP / 2;
        assert_eq!(buf[(source, bend_y)].symbol(), "└");
        assert_eq!(buf[(target, bend_y)].symbol(), "┐");
    }
}
