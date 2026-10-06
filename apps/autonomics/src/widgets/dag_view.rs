//! Interactive read-only DAG canvas backed by the ascii-dag pipeline.
//!
//! ascii-dag owns layout, routing, semantic-cell composition, and hit-test
//! ownership. The Ratatui layer projects those cells into the terminal buffer
//! and adds selection, status colors, scrolling, and the help line.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ascii_dag::render::engine::{
    ArmWeight, CellColor, CellKind, CellMarker, CellView, HitResult, MarkerDirection, NodePaintCtx,
    NodeRegion, SceneComposer, ScenePlanner,
};
use ascii_dag::{CustomNode, Direction, LayoutConfig, PlanOptions, RenderOptions};
use dag_core::dag::{DagTuiSnapshot, RuntimeStatus};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CARD_MIN_WIDTH: usize = 22;
const CARD_MAX_WIDTH: usize = 42;
const CARD_HEIGHT: usize = 5;

/// Selection and viewport state, kept separate from the live graph snapshot.
#[derive(Debug, Clone, Default)]
pub struct DagViewState {
    selected: Option<String>,
    scroll: (u16, u16),
    model: Option<RenderModel>,
}

impl DagViewState {
    pub fn select_up(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_related(snapshot, true);
    }

    pub fn select_down(&mut self, snapshot: &DagTuiSnapshot) {
        self.select_related(snapshot, false);
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
            self.model = None;
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
        let current = self.current_index(snapshot).unwrap_or(0);
        let next = (current + offset) % snapshot.nodes.len();
        self.selected = Some(snapshot.nodes[next].id.clone());
    }

    fn select_sibling(&mut self, snapshot: &DagTuiSnapshot, delta: isize) {
        let Some(current) = self.current_index(snapshot) else {
            return;
        };
        let levels = dag_levels(snapshot);
        let level = levels[current];
        let mut siblings: Vec<usize> = levels
            .iter()
            .enumerate()
            .filter(|(_, node_level)| **node_level == level)
            .map(|(index, _)| index)
            .collect();
        siblings.sort_unstable();
        let Some(position) = siblings.iter().position(|index| *index == current) else {
            return;
        };
        let next = (position as isize + delta).rem_euclid(siblings.len() as isize) as usize;
        self.selected = Some(snapshot.nodes[siblings[next]].id.clone());
    }

    fn select_related(&mut self, snapshot: &DagTuiSnapshot, upstream: bool) {
        let Some(current) = self.current_index(snapshot) else {
            return;
        };
        let selected = &snapshot.nodes[current].id;
        let related: Vec<&str> = snapshot
            .edges
            .iter()
            .filter_map(|edge| {
                let id = if upstream {
                    (&edge.to == selected).then_some(edge.from.as_str())?
                } else {
                    (&edge.from == selected).then_some(edge.to.as_str())?
                };
                snapshot.node(id).map(|node| node.id.as_str())
            })
            .collect();
        if related.is_empty() {
            return;
        }
        let next = if upstream {
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

    /// Reset interaction state when the preview changes fixtures.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.selected = None;
        self.scroll = (0, 0);
        self.model = None;
    }

    fn ensure_model(&mut self, snapshot: &DagTuiSnapshot) -> Option<&RenderModel> {
        let origin = snapshot as *const DagTuiSnapshot as usize;
        let fingerprint = snapshot_fingerprint(snapshot);
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.origin == origin && model.fingerprint == fingerprint)
        {
            self.model = RenderModel::new(snapshot, origin);
        }
        self.model.as_ref()
    }
}

#[derive(Debug, Clone)]
struct CachedCell {
    kind: CellKind,
    color: CellColor,
    owner: HitResult,
}

#[derive(Debug, Clone, Copy)]
struct CachedNode {
    ascii_id: usize,
    center: (usize, usize),
}

/// A cached semantic canvas produced by ascii-dag for one snapshot.
///
/// `LayoutIR` and `Scene` intentionally stay transient: the composer callback
/// copies the public, owned cell vocabulary, so the cache has no borrowed
/// graph strings and remains valid across frames.
#[derive(Debug, Clone)]
struct RenderModel {
    origin: usize,
    fingerprint: u64,
    width: usize,
    height: usize,
    cells: Vec<CachedCell>,
    nodes: Vec<CachedNode>,
    edges: Vec<(usize, usize)>,
    ascii_to_id: HashMap<usize, String>,
    ascii_to_status: HashMap<usize, RuntimeStatus>,
}

impl RenderModel {
    fn new(snapshot: &DagTuiSnapshot, origin: usize) -> Option<Self> {
        let fingerprint = snapshot_fingerprint(snapshot);
        let mut graph = ascii_dag::Graph::new();
        let mut ascii_to_id = HashMap::with_capacity(snapshot.nodes.len());
        let mut ascii_to_status = HashMap::with_capacity(snapshot.nodes.len());
        let mut cards = Vec::with_capacity(snapshot.nodes.len());

        for (index, node) in snapshot.nodes.iter().enumerate() {
            let ascii_id = index + 1;
            let (marker, _) = status_marker(node.status);
            let title = format!("{marker} {}", node.id);
            let width = display_width(&title)
                .max(display_width(&node.kind))
                .max(if node.dirty { 9 } else { 6 })
                .clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH);
            let payload = format!(
                "{}\n{}",
                node.kind,
                if node.dirty { "dirty *" } else { "cached" }
            );
            ascii_to_id.insert(ascii_id, node.id.clone());
            ascii_to_status.insert(ascii_id, node.status);
            cards.push((title, payload, width));
        }

        for (ascii_id, (title, payload, width)) in cards
            .iter()
            .enumerate()
            .map(|(index, card)| (index + 1, card))
        {
            graph.add_node(
                ascii_id,
                CustomNode {
                    label: title.as_str(),
                    width: *width,
                    height: CARD_HEIGHT,
                    painter: Some(dag_card),
                    payload: payload.as_str(),
                },
            );
        }
        for edge in &snapshot.edges {
            let from = snapshot
                .nodes
                .iter()
                .position(|node| node.id == edge.from)
                .map(|index| index + 1);
            let to = snapshot
                .nodes
                .iter()
                .position(|node| node.id == edge.to)
                .map(|index| index + 1);
            if let (Some(from), Some(to)) = (from, to) {
                graph.add_edge(from, to, None);
            }
        }

        let mut config = LayoutConfig::standard();
        config.direction = Direction::LeftRight;
        config.node_spacing = 4;
        config.level_spacing = 2;
        let ir = graph.compute_layout_with_config(&config);

        let mut planner = ScenePlanner::new();
        let scene = planner.plan(&ir, &PlanOptions::new()).quiet().ok()?;
        let requirements = scene.composition_requirements(&RenderOptions::plain().compose);
        let mut composer = SceneComposer::new(requirements);
        let mut cells = Vec::with_capacity(scene.width().saturating_mul(scene.height()));
        composer
            .visit_cells(&scene, |_x, _y, cell: CellView<'_>| {
                cells.push(CachedCell {
                    kind: cell.kind,
                    color: cell.color,
                    owner: cell.owner,
                });
            })
            .ok()?;

        let nodes = scene
            .nodes()
            .map(|node| {
                Some(CachedNode {
                    ascii_id: node.id?,
                    center: (node.x + node.width / 2, node.y + node.height / 2),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let edges = scene
            .edges()
            .map(|edge| (edge.from_id, edge.to_id))
            .collect();
        let result = Self {
            origin,
            fingerprint,
            width: scene.width(),
            height: scene.height(),
            cells,
            nodes,
            edges,
            ascii_to_id,
            ascii_to_status,
        };
        drop(scene);
        Some(result)
    }

    fn selected_ascii_id(&self, selected: &str) -> Option<usize> {
        self.ascii_to_id
            .iter()
            .find(|(_, id)| id.as_str() == selected)
            .map(|(ascii_id, _)| *ascii_id)
    }

    fn selected_edges(&self, selected_ascii_id: usize) -> Vec<usize> {
        self.edges
            .iter()
            .enumerate()
            .filter_map(|(index, (from, to))| {
                (*from == selected_ascii_id || *to == selected_ascii_id).then_some(index)
            })
            .collect()
    }
}

fn dag_card(region: &mut NodeRegion<'_, '_>, ctx: NodePaintCtx<'_>) {
    region.frame();
    let right = region.width().saturating_sub(1);
    region.write_str(2, 1, ctx.label);
    region.hrule(1, right.saturating_sub(1), 2);
    for (row, text) in ctx.payload.lines().enumerate() {
        region.write_str(2, row + 3, text);
    }
}

/// A reusable, immutable snapshot renderer.
#[derive(Debug, Clone, Copy)]
pub struct DagTuiWidget<'a> {
    pub snapshot: &'a DagTuiSnapshot,
    pub focused: bool,
    fixture_help: bool,
}

impl<'a> DagTuiWidget<'a> {
    pub fn new(snapshot: &'a DagTuiSnapshot) -> Self {
        Self {
            snapshot,
            focused: false,
            fixture_help: false,
        }
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    #[allow(dead_code)]
    pub fn fixture_help(mut self, enabled: bool) -> Self {
        self.fixture_help = enabled;
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

        state.ensure_model(self.snapshot);
        let Some(model) = state.model.as_ref() else {
            buf.set_string(
                canvas.left(),
                canvas.top(),
                "DAG layout failed",
                Style::default().fg(Color::Red),
            );
            return;
        };
        let selected_id = state.selected.clone().unwrap_or_default();
        let selected_ascii = model.selected_ascii_id(&selected_id);
        let selected_edges = selected_ascii
            .map(|id| model.selected_edges(id))
            .unwrap_or_default();
        let virtual_size = (model.width, model.height);
        let focus = selected_ascii.and_then(|id| {
            model
                .nodes
                .iter()
                .find(|node| node.ascii_id == id)
                .map(|node| node.center)
        });
        fit_viewport(state, canvas, virtual_size, focus);

        let Some(model) = state.model.as_ref() else {
            return;
        };

        let (scroll_x, scroll_y) = state.scroll;
        for (index, cell) in model.cells.iter().enumerate() {
            let x = index % model.width;
            let y = index / model.width;
            let (Some(screen_x), Some(screen_y)) = (
                as_screen(canvas.x, canvas.width, x, scroll_x),
                as_screen(canvas.y, canvas.height, y, scroll_y),
            ) else {
                continue;
            };
            if matches!(cell.kind, CellKind::Empty) {
                continue;
            }
            let ch = decode_cell(cell.kind);
            buf.set_string(
                screen_x,
                screen_y,
                ch.to_string(),
                cell_style(
                    cell,
                    selected_ascii,
                    &selected_edges,
                    &model.ascii_to_status,
                ),
            );
        }

        render_status_line(
            buf,
            area,
            self.snapshot,
            state,
            self.focused,
            self.fixture_help,
        );
    }
}

fn fit_viewport(
    state: &mut DagViewState,
    canvas: Rect,
    virtual_size: (usize, usize),
    focus: Option<(usize, usize)>,
) {
    state.scroll.0 = fit_axis(
        state.scroll.0,
        canvas.width,
        virtual_size.0,
        focus.map(|(x, _)| x),
    );
    state.scroll.1 = fit_axis(
        state.scroll.1,
        canvas.height,
        virtual_size.1,
        focus.map(|(_, y)| y),
    );
}

fn fit_axis(current: u16, viewport: u16, virtual_size: usize, focus: Option<usize>) -> u16 {
    let Some(virtual_size) = u16::try_from(virtual_size).ok() else {
        return current;
    };
    if viewport == 0 || virtual_size <= viewport {
        return 0;
    }
    let max = virtual_size - viewport;
    let desired = focus
        .and_then(|position| u16::try_from(position).ok())
        .map(|position| position.saturating_sub(viewport / 2))
        .unwrap_or(current);
    desired.min(max)
}

fn as_screen(origin: u16, size: u16, value: usize, scroll: u16) -> Option<u16> {
    let value = u16::try_from(value).ok()?;
    if value < scroll {
        return None;
    }
    let relative = value - scroll;
    (relative < size).then_some(origin + relative)
}

fn cell_style(
    cell: &CachedCell,
    selected_ascii: Option<usize>,
    selected_edges: &[usize],
    statuses: &HashMap<usize, RuntimeStatus>,
) -> Style {
    match cell.owner {
        HitResult::Node(id) => {
            if Some(id) == selected_ascii {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                let (_, color) = statuses
                    .get(&id)
                    .map(|status| status_marker(*status))
                    .unwrap_or(("○", Color::Gray));
                Style::default().fg(color)
            }
        }
        HitResult::Edge(index) if selected_edges.contains(&index) => {
            Style::default().fg(Color::Yellow)
        }
        HitResult::Edge(_) => Style::default().fg(cell_color(cell.color)),
        HitResult::Subgraph(_) => Style::default().fg(Color::DarkGray),
        HitResult::Dummy { .. } => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Gray),
    }
}

fn cell_color(color: CellColor) -> Color {
    color
        .as_rgb()
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::DarkGray)
}

fn decode_cell(kind: CellKind) -> char {
    match kind {
        CellKind::Empty => ' ',
        CellKind::Text { ch } => ch,
        CellKind::Marker { marker } => marker_char(marker),
        CellKind::Stroke { arms } => stroke_char(
            arm(arms.up),
            arm(arms.down),
            arm(arms.left),
            arm(arms.right),
        ),
        _ => ' ',
    }
}

fn marker_char(marker: CellMarker) -> char {
    match marker {
        CellMarker::SelfLoop => '↺',
        CellMarker::Dummy => '◍',
        CellMarker::Arrow { direction, dashed } => match (direction, dashed) {
            (MarkerDirection::Down, false) => '↓',
            (MarkerDirection::Down, true) => '⇣',
            (MarkerDirection::Up, false) => '↑',
            (MarkerDirection::Up, true) => '⇡',
            (MarkerDirection::Right, false) => '→',
            (MarkerDirection::Right, true) => '⇢',
            (MarkerDirection::Left, false) => '←',
            (MarkerDirection::Left, true) => '⇠',
        },
        _ => ' ',
    }
}

fn arm(weight: ArmWeight) -> u8 {
    match weight {
        ArmWeight::None => 0,
        ArmWeight::Dashed => 1,
        ArmWeight::Light | ArmWeight::Double => 2,
        _ => 2,
    }
}

fn stroke_char(up: u8, down: u8, left: u8, right: u8) -> char {
    if up == 1 && down == 1 && left == 0 && right == 0 {
        return '┊';
    }
    if up == 0 && down == 0 && left == 1 && right == 1 {
        return '┈';
    }
    let light = up == 2 || down == 2 || left == 2 || right == 2;
    let fold = |value: u8| {
        if value == 1 && light { 2 } else { value }
    };
    match (fold(up), fold(down), fold(left), fold(right)) {
        (0, 2, 0, 0) | (2, 0, 0, 0) | (2, 2, 0, 0) => '│',
        (0, 0, 2, 0) | (0, 0, 0, 2) | (0, 0, 2, 2) => '─',
        (0, 2, 0, 2) => '┌',
        (0, 2, 2, 0) => '┐',
        (2, 0, 0, 2) => '└',
        (2, 0, 2, 0) => '┘',
        (2, 2, 0, 2) => '├',
        (2, 2, 2, 0) => '┤',
        (0, 2, 2, 2) => '┬',
        (2, 0, 2, 2) => '┴',
        (2, 2, 2, 2) => '┼',
        _ => {
            let mut mask = 0;
            if up != 0 {
                mask |= 1;
            }
            if down != 0 {
                mask |= 2;
            }
            if left != 0 {
                mask |= 4;
            }
            if right != 0 {
                mask |= 8;
            }
            match mask {
                1..=3 => '│',
                4 | 8 | 12 => '─',
                6 => '┌',
                5 => '┐',
                10 => '└',
                9 => '┘',
                7 => '├',
                14 => '┤',
                13 => '┬',
                11 => '┴',
                15 => '┼',
                _ => ' ',
            }
        }
    }
}

fn render_status_line(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    snapshot: &DagTuiSnapshot,
    state: &DagViewState,
    focused: bool,
    fixture_help: bool,
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
        let fixture_hint = if fixture_help {
            " · Space fixture"
        } else {
            ""
        };
        spans.push(Span::styled(
            format!("  ←→ dataflow · ↑↓ siblings · Tab next{fixture_hint} · R refresh · Esc close"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let line = Line::from(spans);
    let y = area.bottom().saturating_sub(1);
    let mut x = area.left();
    for span in line.spans {
        if x >= area.right() {
            break;
        }
        let text = truncate(&span.content, (area.right() - x) as usize);
        if text.is_empty() {
            continue;
        }
        let width = text.width() as u16;
        buf.set_string(x, y, text, span.style);
        x = x.saturating_add(width);
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
        RuntimeStatus::Cancelled => ("⊘", Color::Yellow),
    }
}

fn dag_levels(snapshot: &DagTuiSnapshot) -> Vec<usize> {
    let mut levels = vec![0usize; snapshot.nodes.len()];
    let mut position = HashMap::with_capacity(snapshot.nodes.len());
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

fn truncate(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut width = 0usize;
    for grapheme in UnicodeSegmentation::graphemes(value, true) {
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

fn display_width(value: &str) -> usize {
    UnicodeSegmentation::graphemes(value, true)
        .map(UnicodeWidthStr::width)
        .sum()
}

fn snapshot_fingerprint(snapshot: &DagTuiSnapshot) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{snapshot:?}").hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dag_core::dag::{DagEdgeView, DagNodeView};
    use ratatui::buffer::Buffer;

    fn node(id: &str, status: RuntimeStatus, dirty: bool) -> DagNodeView {
        DagNodeView {
            id: id.to_string(),
            kind: "echo".to_string(),
            status,
            dirty,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn edge(from: &str, to: &str) -> DagEdgeView {
        DagEdgeView {
            from: from.to_string(),
            from_port: 0,
            to: to.to_string(),
            to_port: 0,
        }
    }

    fn snapshot() -> DagTuiSnapshot {
        DagTuiSnapshot {
            nodes: vec![
                node("ingest", RuntimeStatus::Success, false),
                node("validate", RuntimeStatus::Success, false),
                node("transform", RuntimeStatus::Running, true),
                node("report", RuntimeStatus::Pending, true),
            ],
            edges: vec![
                edge("ingest", "validate"),
                edge("ingest", "transform"),
                edge("validate", "report"),
                edge("transform", "report"),
            ],
        }
    }

    #[test]
    fn selection_follows_dataflow_and_wraps() {
        let graph = snapshot();
        let mut state = DagViewState::default();
        state.select_next(&graph);
        assert_eq!(state.selected.as_deref(), Some("ingest"));
        state.select_down(&graph);
        assert_eq!(state.selected.as_deref(), Some("transform"));
        state.select_up(&graph);
        assert_eq!(state.selected.as_deref(), Some("ingest"));
        state.select_previous(&graph);
        assert_eq!(state.selected.as_deref(), Some("report"));
    }

    #[test]
    fn ascii_dag_pipeline_renders_semantic_cells() {
        let mut state = DagViewState::default();
        state.select_next(&snapshot());
        let model = state.ensure_model(&snapshot()).expect("model");
        assert!(model.width > 0);
        assert!(model.cells.len() > 100);
        assert!(model.cells.iter().any(|cell| {
            matches!(
                cell.kind,
                CellKind::Text { ch: 'i' } | CellKind::Stroke { .. }
            )
        }));
    }

    #[test]
    fn widget_projects_composed_cells_to_ratatui_buffer() {
        let graph = snapshot();
        let mut state = DagViewState::default();
        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);

        DagTuiWidget::new(&graph)
            .focused(true)
            .render(area, &mut buf, &mut state);

        assert!(buf.content.iter().any(|cell| cell.symbol() == "i"));
        assert!(buf.content.iter().any(|cell| {
            matches!(cell.symbol(), "─" | "│" | "┌" | "┐" | "└" | "┘")
        }));
    }
}
