//! Sidebar showing the agent's task plan, background tasks, and a docked
//! switch-agent panel. Frameless and untitled — no outer top or bottom
//! border rows and no section headers — so the lists keep every row: the
//! plan box carries only side rules, and each lower box tops with a single
//! bare divider line separating it from the one above. Populated sections
//! identify themselves through their rows. Each box's rules light up (cyan)
//! only when its own pane holds focus, mirroring the chat pane's left-rule
//! indicator.

use ratatui::{
    layout::{Alignment, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::state::{AgentStatus, PlanState, ToolTaskInfo, ToolTaskStatus, WorkspaceFocus};
use crate::widgets::agent_filter::AgentFilterState;
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};

/// One running agent entry in the docked switch-agent panel, supplied by
/// the app on every frame (statuses are live).
#[derive(Clone, Debug)]
pub struct RunningAgent {
    pub name: String,
    pub agent_id: uuid::Uuid,
    pub status: AgentStatus,
    pub is_active: bool,
    /// The agent's live model spec (`provider:model`) from the render cache;
    /// `None` while the per-agent fetch is still in flight (column blank).
    pub model: Option<String>,
    /// The reasoning effort this agent's next request would send
    /// (`low | … | ultra`); `None` on non-thinking models (column `–`) and
    /// alongside a `None` model (column blank).
    pub effort: Option<String>,
}

impl RunningAgent {
    /// The text shown for this agent: the workspace-root-stripped path.
    pub fn shown_name(&self) -> &str {
        display_name(&self.name)
    }
}

/// Order running agents shallow-path-first, then lexically — a stable order
/// across frames.
pub fn sort_running_agents(agents: &mut [RunningAgent]) {
    agents.sort_by(|left, right| {
        let depth = |name: &str| name.split('/').filter(|part| !part.is_empty()).count();
        depth(&left.name)
            .cmp(&depth(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// The agents-panel column the cursor acts on: `h`/`l` move it, Enter
/// dispatches per column (name switches, model opens the picker, effort
/// cycles), and mouse clicks land on one of the three cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentColumn {
    #[default]
    Name,
    Model,
    Effort,
}

/// Which cell of an agents-panel row a click hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCell {
    /// Anywhere outside the model/effort cells: the row's name area.
    Row,
    Model,
    Effort,
}

/// Clickable regions of one agents-panel row: the full row plus the model
/// and effort cells recorded from the last render. The cells are `None`
/// when the sidebar is too narrow to show that column.
#[derive(Debug, Clone)]
pub struct AgentRowHit {
    pub row: Rect,
    pub id: uuid::Uuid,
    pub model: Option<Rect>,
    pub effort: Option<Rect>,
}

/// Fixed widths of the trailing agents-panel columns. The effort column
/// fits the longest common effort id (`medium`).
const AGENT_MODEL_COL: u16 = 12;
const AGENT_EFFORT_COL: u16 = 6;

/// Column budgets for one agents-panel row: the name flexes, the model and
/// effort columns are fixed and drop out (effort first) on narrow sidebars
/// so the name keeps a usable share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentRowLayout {
    pub name: u16,
    pub model: Option<u16>,
    pub effort: Option<u16>,
}

/// `item_width` is one List item's content width (the section's inner width
/// minus the 2-column highlight gutter).
pub(crate) fn agent_row_layout(item_width: u16) -> AgentRowLayout {
    // The row's own prefix "● " takes 2 columns before the name.
    let avail = item_width.saturating_sub(2);
    let model_with_sep = AGENT_MODEL_COL + 1;
    let effort_with_sep = AGENT_EFFORT_COL + 1;
    if avail >= model_with_sep + effort_with_sep + 6 {
        AgentRowLayout {
            name: avail - model_with_sep - effort_with_sep,
            model: Some(AGENT_MODEL_COL),
            effort: Some(AGENT_EFFORT_COL),
        }
    } else if avail >= model_with_sep + 3 {
        AgentRowLayout {
            name: avail - model_with_sep,
            model: Some(AGENT_MODEL_COL),
            effort: None,
        }
    } else {
        AgentRowLayout {
            name: avail,
            model: None,
            effort: None,
        }
    }
}

#[derive(Default)]
pub struct SidebarState {
    pub plan: ListState,
    pub tasks: ListState,
    pub agents: ListState,
    pub agent_filter: AgentFilterState,
    /// The agents-panel column cursor (`h`/`l` move it; Enter dispatches
    /// per column).
    pub agent_column: AgentColumn,
    selected_task_seq: Option<u64>,
    selected_agent: Option<uuid::Uuid>,
    /// The agents list's last rendered width, so key-time column moves can
    /// skip columns the current width hides. `pub(crate)` lets key-handling
    /// tests prime it without a render pass.
    pub(crate) agents_list_width: u16,
    /// Clickable row regions of the agents panel from the last render
    /// (screen-space rects paired with the agent they act on, plus the
    /// model/effort cells when those columns are visible).
    pub agent_row_areas: Vec<AgentRowHit>,
}

impl SidebarState {
    /// Keep selection attached to the same task when completed rows disappear.
    pub fn sync(&mut self, plan_len: usize, tasks: &[ToolTaskInfo]) {
        self.plan
            .select((plan_len > 0).then(|| self.plan.selected().unwrap_or(0).min(plan_len - 1)));
        let selected = self
            .selected_task_seq
            .and_then(|seq| tasks.iter().position(|task| task.seq == seq))
            .or_else(|| {
                (!tasks.is_empty()).then(|| self.tasks.selected().unwrap_or(0).min(tasks.len() - 1))
            });
        self.select_task(selected, tasks);
    }

    pub fn select_task(&mut self, selected: Option<usize>, tasks: &[ToolTaskInfo]) {
        let selected = selected.filter(|&index| index < tasks.len());
        self.tasks.select(selected);
        self.selected_task_seq = selected.map(|index| tasks[index].seq);
    }

    /// Resolve the agents-panel cursor to a row index: the last selected
    /// agent if still running, else the active leaf (so Enter on a fresh
    /// panel is a no-op confirmation). Entering the panel clears the stored
    /// id (see `focus_pane`), so every arrival re-anchors to the active
    /// leaf; the stored id only persists while the panel keeps focus.
    pub fn selected_agent_index(&self, agents: &[RunningAgent]) -> Option<usize> {
        self.selected_agent
            .and_then(|id| {
                agents
                    .iter()
                    .position(|agent| agent.agent_id == id && self.agent_filter.matches(agent))
            })
            .or_else(|| {
                agents
                    .iter()
                    .position(|agent| agent.is_active && self.agent_filter.matches(agent))
            })
            .or_else(|| {
                agents
                    .iter()
                    .position(|agent| self.agent_filter.matches(agent))
            })
    }

    pub fn filtered_agents(&self, agents: &[RunningAgent]) -> Vec<RunningAgent> {
        agents
            .iter()
            .filter(|agent| self.agent_filter.matches(agent))
            .cloned()
            .collect()
    }

    pub fn reset_agent_scroll(&mut self) {
        self.agents = ListState::default();
        self.agent_row_areas.clear();
        self.agent_column = AgentColumn::Name;
    }

    pub fn clear_agent_hit_areas(&mut self) {
        self.agent_row_areas.clear();
        self.agent_filter.clear_hit_areas();
    }

    /// The agent the panel's Enter / Ctrl+W would act on.
    pub fn selected_agent_id(&self, agents: &[RunningAgent]) -> Option<uuid::Uuid> {
        self.selected_agent_index(agents)
            .map(|i| agents[i].agent_id)
    }

    /// Move the panel cursor to `index`, anchored by agent identity so the
    /// selection survives spawns/closes/reorders between frames.
    pub fn select_agent(&mut self, agents: &[RunningAgent], index: usize) {
        self.selected_agent = agents.get(index).map(|agent| agent.agent_id);
    }

    /// Drop the stored agents-panel cursor so the resolver falls back to
    /// the active leaf. Entering the panel calls this (see `focus_pane`).
    pub fn clear_agent_selection(&mut self) {
        self.selected_agent = None;
    }

    /// Move the column cursor one column left/right. Columns the last
    /// rendered width hides are skipped; the cursor is sticky at Name and
    /// the last visible column (vim-style, no wrap).
    pub fn advance_agent_column(&mut self, right: bool) {
        let layout = agent_row_layout(self.agents_list_width.saturating_sub(2));
        let visible = |column: AgentColumn| match column {
            AgentColumn::Name => true,
            AgentColumn::Model => layout.model.is_some(),
            AgentColumn::Effort => layout.effort.is_some(),
        };
        let step = |column: &mut AgentColumn| match (*column, right) {
            (AgentColumn::Name, true) => *column = AgentColumn::Model,
            (AgentColumn::Model, false) => *column = AgentColumn::Name,
            (AgentColumn::Model, true) => *column = AgentColumn::Effort,
            (AgentColumn::Effort, false) => *column = AgentColumn::Model,
            (AgentColumn::Name, false) | (AgentColumn::Effort, true) => {}
        };
        let mut next = self.agent_column;
        step(&mut next);
        if !visible(next) {
            return;
        }
        self.agent_column = next;
    }

    /// Route a click to an agents-panel row; returns the clicked agent and
    /// the cell it landed on. The row cursor and column cursor both move to
    /// the click target.
    pub fn click_agent_cell(&mut self, x: u16, y: u16) -> Option<(uuid::Uuid, AgentCell)> {
        let contains =
            |area: Rect| x >= area.x && x < area.right() && y >= area.y && y < area.bottom();
        let hit = self.agent_row_areas.iter().find(|hit| contains(hit.row))?;
        let cell = if hit.model.is_some_and(contains) {
            AgentCell::Model
        } else if hit.effort.is_some_and(contains) {
            AgentCell::Effort
        } else {
            AgentCell::Row
        };
        self.selected_agent = Some(hit.id);
        self.agent_column = match cell {
            AgentCell::Row => AgentColumn::Name,
            AgentCell::Model => AgentColumn::Model,
            AgentCell::Effort => AgentColumn::Effort,
        };
        Some((hit.id, cell))
    }
}

/// Data bundle the sidebar needs from the agent tab state.
pub struct SidebarData<'a> {
    pub plan: &'a PlanState,
    pub tool_tasks: &'a [ToolTaskInfo],
    /// All running agents (workspace-wide, not just this leaf) for the
    /// docked switch-agent panel.
    pub agents: &'a [RunningAgent],
}

/// Fit populated sections to their content. When space is scarce, keep the
/// focused list usable first, then share remaining rows between visible
/// lists. `rows` includes agent search controls, but excludes inter-section
/// dividers.
fn sidebar_areas(area: Rect, focus: WorkspaceFocus, rows: [usize; 3]) -> [Rect; 3] {
    let focused = match focus {
        WorkspaceFocus::Plan => 0,
        WorkspaceFocus::Tasks => 1,
        _ => 2,
    };
    let mut priority = vec![focused];
    for index in [2, 0, 1] {
        if !priority.contains(&index) {
            priority.push(index);
        }
    }
    let mut heights = [0u16; 3];
    let mut remaining = area.height;
    let mut visible = 0;
    for &index in &priority {
        if rows[index] == 0 || remaining == 0 {
            continue;
        }
        let divider = u16::from(visible > 0);
        let minimum = if index == 2 { 3 } else { 1 };
        if remaining >= minimum + divider {
            heights[index] = minimum;
            remaining -= minimum + divider;
            visible += 1;
        } else if visible == 0 {
            // Extremely short windows still expose the focused controls.
            heights[index] = remaining;
            remaining = 0;
            visible += 1;
        }
    }
    while remaining > 0 {
        let mut grew = false;
        for &index in &priority {
            if heights[index] > 0 && usize::from(heights[index]) < rows[index] {
                heights[index] += 1;
                remaining -= 1;
                grew = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut y = area.y;
    let mut first = true;
    heights.map(|height| {
        let divider = u16::from(!first && height > 0);
        let section = Rect::new(area.x, y, area.width, height + divider);
        y += section.height;
        first &= height == 0;
        section
    })
}

/// Right-hand sidebar widget showing the plan, background tasks, and the
/// docked switch-agent panel.
pub struct SideBar<'a> {
    pub data: SidebarData<'a>,
    pub focus: WorkspaceFocus,
}

impl StatefulWidget for SideBar<'_> {
    type State = SidebarState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.sync(self.data.plan.steps.len(), self.data.tool_tasks);
        let plan_focused = self.focus == WorkspaceFocus::Plan;
        let tasks_focused = self.focus == WorkspaceFocus::Tasks;
        let agents_focused = self.focus == WorkspaceFocus::Agents;

        // Click targets are recomputed from scratch every frame.
        state.clear_agent_hit_areas();
        let filtered = state.filtered_agents(self.data.agents);
        let agent_rows = if self.data.agents.is_empty() {
            state.agents = ListState::default();
            0
        } else {
            // Keep the controls available when a filter has no matches.
            2 + filtered.len().max(1)
        };
        let [plan_area, tasks_area, agents_area] = sidebar_areas(
            area,
            self.focus,
            [
                self.data.plan.steps.len(),
                self.data.tool_tasks.len(),
                agent_rows,
            ],
        );
        let borders = |section: Rect| {
            Borders::LEFT
                | Borders::RIGHT
                | if section.y > area.y {
                    Borders::TOP
                } else {
                    Borders::NONE
                }
        };

        // ── Box 1: plan (side rules only; content starts on the first row) ──
        let plan_block =
            Block::default()
                .borders(borders(plan_area))
                .border_style(Style::default().fg(if plan_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }));
        let plan_inner = plan_block.inner(plan_area);
        plan_block.render(plan_area, buf);
        if plan_inner.height > 0 {
            render_plan_section(
                plan_inner,
                buf,
                self.data.plan,
                &mut state.plan,
                plan_focused,
            );
        }

        // ── Box 2: tasks (bare top divider; open bottom) ──
        let tasks_block =
            Block::default()
                .borders(borders(tasks_area))
                .border_style(Style::default().fg(if tasks_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }));
        let tasks_inner = tasks_block.inner(tasks_area);
        tasks_block.render(tasks_area, buf);
        if tasks_inner.height > 0 {
            render_task_section(
                tasks_inner,
                buf,
                self.data.tool_tasks,
                &mut state.tasks,
                tasks_focused,
            );
        }

        // ── Box 3: docked switch-agent panel ──
        if !agents_area.is_empty() {
            let agents_block =
                Block::default()
                    .borders(borders(agents_area))
                    .border_style(Style::default().fg(if agents_focused {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }));
            let agents_inner = agents_block.inner(agents_area);
            agents_block.render(agents_area, buf);
            if agents_inner.height > 0 {
                let controls = state
                    .agent_filter
                    .render(agents_inner, buf, self.data.agents, agents_focused)
                    .min(agents_inner.height);
                let list_area = Rect {
                    y: agents_inner.y + controls,
                    height: agents_inner.height - controls,
                    ..agents_inner
                };
                // Resolve the selected row before the call: `&mut
                // state.agents` is an argument, so its borrow spans the
                // whole call and cannot share the call with `&*state`.
                let selected_row = state.selected_agent_index(&filtered);
                state.agents_list_width = list_area.width;
                if filtered.is_empty() && !self.data.agents.is_empty() {
                    state.agents = ListState::default();
                    render_placeholder(list_area, buf, "(no matching agents)");
                } else {
                    render_agents_section(
                        list_area,
                        buf,
                        &filtered,
                        &mut state.agents,
                        selected_row,
                        &mut state.agent_row_areas,
                        agents_focused,
                        state.agent_column,
                    );
                }
            }
        }
    }
}

// ── Section renderers ──────────────────────────────────

/// Render the plan steps list, or a placeholder if empty.
fn render_plan_section(
    area: Rect,
    buf: &mut Buffer,
    plan: &PlanState,
    state: &mut ListState,
    focused: bool,
) {
    if area.height == 0 {
        return;
    }
    if plan.is_empty() {
        render_placeholder(area, buf, "(no plan)");
        return;
    }
    let rows = plan
        .steps
        .iter()
        .map(|step| ListItem::new(plan_step_line(step, area.width.saturating_sub(2))));
    StatefulWidget::render(
        List::new(rows)
            .highlight_symbol(if focused { "› " } else { "  " })
            .highlight_style(selection_style(focused)),
        area,
        buf,
        state,
    );
}

/// Render the background tasks list, or a placeholder if empty.
fn render_task_section(
    area: Rect,
    buf: &mut Buffer,
    tasks: &[ToolTaskInfo],
    state: &mut ListState,
    focused: bool,
) {
    if area.height == 0 {
        return;
    }
    if tasks.is_empty() {
        render_placeholder(area, buf, "(no tasks)");
        return;
    }
    let rows = tasks
        .iter()
        .map(|task| ListItem::new(task_line(task, area.width.saturating_sub(2))));
    StatefulWidget::render(
        List::new(rows)
            .highlight_symbol(if focused { "› " } else { "  " })
            .highlight_style(selection_style(focused)),
        area,
        buf,
        state,
    );
}

/// Render the docked switch-agent list. Rows are status icon + display name
/// (bold red name on the active leaf) with trailing model and effort
/// columns that drop out on narrow sidebars; selection follows the
/// id-anchored cursor and the List keeps it scrolled into view. When more
/// agents exist than rows, the last row becomes a muted `+N more`
/// overflow hint.
#[allow(clippy::too_many_arguments)]
fn render_agents_section(
    area: Rect,
    buf: &mut Buffer,
    agents: &[RunningAgent],
    list_state: &mut ListState,
    selected: Option<usize>,
    row_areas: &mut Vec<AgentRowHit>,
    focused: bool,
    column: AgentColumn,
) {
    if area.is_empty() {
        return;
    }
    if agents.is_empty() {
        *list_state = ListState::default();
        render_placeholder(area, buf, "(no agents)");
        return;
    }

    let selected = selected.unwrap_or(0);
    list_state.select(Some(selected));

    // Reserve the final row for the overflow hint when the list cannot fit.
    let overflowing = area.height > 1 && agents.len() > usize::from(area.height);
    let list_height = if overflowing {
        area.height.saturating_sub(1)
    } else {
        area.height
    };
    let hidden = agents.len().saturating_sub(usize::from(list_height));
    let list_area = Rect {
        height: list_height,
        ..area
    };
    let layout = agent_row_layout(area.width.saturating_sub(2));

    let items: Vec<ListItem> = agents
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            ListItem::new(agent_row_line(
                agent,
                i == selected,
                focused,
                column,
                layout,
            ))
        })
        .collect();
    StatefulWidget::render(
        List::new(items)
            .highlight_symbol(if focused { "› " } else { "  " })
            .highlight_style(selection_style(focused)),
        list_area,
        buf,
        list_state,
    );

    // Record clickable row regions: whatever the List actually shows. Cell
    // x-offsets mirror the spans in `agent_row_line` (gutter + prefix +
    // name budget), so they stay row-invariant.
    let offset = list_state.offset();
    let model_cell = layout
        .model
        .map(|_| Rect::new(area.x + 2 + 2 + layout.name + 1, area.y, AGENT_MODEL_COL, 1));
    let effort_cell = layout.effort.map(|_| {
        Rect::new(
            area.x + 2 + 2 + layout.name + 1 + AGENT_MODEL_COL + 1,
            area.y,
            AGENT_EFFORT_COL,
            1,
        )
    });
    for (i, agent) in agents
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_height as usize)
    {
        let y = area.y + (i - offset) as u16;
        row_areas.push(AgentRowHit {
            row: Rect::new(area.x, y, area.width, 1),
            id: agent.agent_id,
            model: model_cell.map(|cell| Rect { y, ..cell }),
            effort: effort_cell.map(|cell| Rect { y, ..cell }),
        });
    }

    if hidden > 0 {
        let hint_area = Rect {
            y: area.y + list_height,
            height: area.height - list_height,
            ..area
        };
        Paragraph::new(Line::from(Span::styled(
            format!("  +{hidden} more"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )))
        .render(hint_area, buf);
    }
}

/// One switch-agent row: status icon + display name (workspace-root prefix
/// stripped, bold red name on the active leaf, kept inside the name budget)
/// with trailing model and effort columns. Cell x-offsets are row-invariant
/// so the columns and their click targets stay aligned; the column cursor
/// underlines its cell on the selected row.
fn agent_row_line(
    agent: &RunningAgent,
    selected: bool,
    focused: bool,
    column: AgentColumn,
    layout: AgentRowLayout,
) -> Line<'static> {
    let icon = status_icon(&agent.status);
    let color = status_color(&agent.status);
    let highlighted = selected && focused;
    // The model/effort cells sit on the row's selection band with the name.
    // The band itself is the List's highlight_style, so these spans must
    // not paint a background of their own; they choose bright foregrounds
    // for that band instead.
    let cell_style = if highlighted {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let (icon_style, name_style) = if highlighted {
        (
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(color),
            if agent.is_active {
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };

    // The cell always pads out to the full name budget, keeping the model
    // column aligned when the truncated name ends short.
    let name_width = layout.name as usize;
    let name_text = truncate_str(agent.shown_name(), name_width);
    let used = name_text.chars().count();

    let mut spans = vec![
        Span::styled(icon, icon_style),
        Span::raw(" "),
        Span::styled(name_text, name_style),
    ];
    spans.push(Span::raw(" ".repeat(name_width.saturating_sub(used))));

    if let Some(model_width) = layout.model {
        let model_text = model_cell_text(agent.model.as_deref(), model_width as usize);
        // The underline marks only the cursor's own cell.
        let model_style = if focused && selected && column == AgentColumn::Model {
            cell_style.add_modifier(Modifier::UNDERLINED)
        } else {
            cell_style
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(model_text, model_style));
    }
    if layout.effort.is_some() {
        let effort_text = match (&agent.model, &agent.effort) {
            // Known non-thinking model: a visible dash reads as "none".
            (Some(_), None) => "–".to_string(),
            (Some(_), Some(effort)) => truncate_str(effort, AGENT_EFFORT_COL as usize),
            // Model unknown (fetch in flight / daemon down): blank.
            (None, _) => String::new(),
        };
        let effort_style = if focused && selected && column == AgentColumn::Effort {
            cell_style.add_modifier(Modifier::UNDERLINED)
        } else {
            cell_style
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(effort_text, effort_style));
    }
    Line::from(spans)
}

/// Fit a model spec into the model column: verbatim when it fits, else the
/// `model` part alone (dropping the provider before the **first** colon —
/// ollama tags like `qwen3:32b` carry colons of their own), else that part
/// truncated.
fn model_cell_text(spec: Option<&str>, width: usize) -> String {
    let Some(spec) = spec else {
        return String::new();
    };
    if spec.chars().count() <= width {
        return spec.to_string();
    }
    let tail = spec.split_once(':').map(|(_, model)| model).unwrap_or(spec);
    truncate_str(tail, width)
}

fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .bg(Color::Rgb(35, 55, 70))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

// ── Row renderers ──────────────────────────────────────

/// Render a single plan step row.
fn plan_step_line(step: &agentik_types::PlanStep, width: u16) -> Line<'static> {
    let (icon, style) = match step.status {
        agentik_types::StepStatus::Completed => (
            "✔ ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        ),
        agentik_types::StepStatus::InProgress => (
            "▶ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        agentik_types::StepStatus::Pending => ("○ ", Style::default().fg(Color::DarkGray)),
    };

    let icon_width = 2;
    let max_text_len = width.saturating_sub(icon_width) as usize;
    let text = truncate_str(&step.step, max_text_len);

    Line::from(vec![Span::styled(icon, style), Span::styled(text, style)])
}

/// Render a single background task row.
fn task_line(task: &ToolTaskInfo, width: u16) -> Line<'static> {
    let (icon, color) = match &task.status {
        ToolTaskStatus::Running => ("⏳", Color::Yellow),
        ToolTaskStatus::Done { ok: true } => ("✓", Color::Green),
        ToolTaskStatus::Done { ok: false } => ("✗", Color::Red),
    };

    let icon_width = 3; // "⏳ " = 3 cells (hourglass renders double-width)
    let max_text_len = width.saturating_sub(icon_width) as usize;
    let label = match &task.target_agent {
        Some(target) if task.name == "delegate_to" => format!("#{} → {}", task.seq, target),
        Some(target) => format!("#{} {} → {}", task.seq, task.name, target),
        None => format!("#{} {}", task.seq, task.name),
    };
    let text = truncate_str(&label, max_text_len);

    Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            text,
            Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render a muted placeholder centered in the section body.
fn render_placeholder(area: Rect, buf: &mut Buffer, text: &str) {
    let line = Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM | Modifier::ITALIC),
    ));
    Paragraph::new(line)
        .alignment(Alignment::Center)
        .render(area, buf);
}

/// Truncate a string to `max_len` characters, appending `…` if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else if max_len == 0 {
        String::new()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(seq: u64) -> ToolTaskInfo {
        ToolTaskInfo {
            seq,
            name: "delegate_to".into(),
            status: ToolTaskStatus::Running,
            target_agent: Some(format!("worker-{seq}")),
        }
    }

    fn running(name: &str, active: bool) -> RunningAgent {
        RunningAgent {
            name: name.to_string(),
            agent_id: uuid::Uuid::new_v4(),
            status: AgentStatus::Idle,
            is_active: active,
            model: None,
            effort: None,
        }
    }

    #[test]
    fn agent_cursor_survives_reorder_and_falls_back_to_active() {
        let mut state = SidebarState::default();
        let a = running("/root/a", false);
        let b = running("/root/b", true);
        let mut agents = vec![a.clone(), b.clone()];
        state.select_agent(&agents, 0);
        // Reorder (sort puts shallow/lexical first — same order here), then
        // drop the selected agent: the resolver falls back to the active.
        agents.remove(0);
        assert_eq!(state.selected_agent_index(&agents), Some(0));
        assert_eq!(state.selected_agent_id(&agents), Some(b.agent_id));
    }

    #[test]
    fn column_moves_skip_hidden_columns_and_do_not_wrap() {
        let mut state = SidebarState::default();
        // Narrow width: only the name column renders.
        state.agents_list_width = 10;
        state.advance_agent_column(true);
        assert_eq!(state.agent_column, AgentColumn::Name, "model hidden at 10");
        // Wide enough for all three columns.
        state.agents_list_width = 40;
        state.advance_agent_column(true);
        assert_eq!(state.agent_column, AgentColumn::Model);
        state.advance_agent_column(true);
        assert_eq!(state.agent_column, AgentColumn::Effort);
        state.advance_agent_column(true);
        assert_eq!(state.agent_column, AgentColumn::Effort, "sticky at the end");
        state.advance_agent_column(false);
        state.advance_agent_column(false);
        assert_eq!(state.agent_column, AgentColumn::Name);
        state.advance_agent_column(false);
        assert_eq!(state.agent_column, AgentColumn::Name, "sticky at the start");
    }

    #[test]
    fn task_rows_name_delegation_targets() {
        let line = task_line(&task(7), 40);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect();
        assert!(text.contains("#7 → worker-7"), "got: {text}");
    }

    #[test]
    fn model_cell_drops_provider_before_the_first_colon() {
        assert_eq!(model_cell_text(Some("zai:glm-5.3"), 6), "glm-5…");
        assert_eq!(model_cell_text(Some("short"), 8), "short");
        assert_eq!(model_cell_text(None, 8), "");
    }
}
