//! Skill-evolution dashboard: the loop's control surface in the TUI.
//!
//! One overlay shows the whole picture — service configuration,
//! library generation, observation counters — above a two-pane
//! **skill browser**: the left list is the unified library (every
//! installed skill across tiers plus every proposed-but-not-installed
//! name, the proposal pipeline being one attribute among others); the
//! right pane is the selected skill's detail document — metadata,
//! usage telemetry, proposal status with its evidence, and the full
//! SKILL.md body. ←/→ (h/l) move focus between the panes; the focused
//! pane owns the vertical keys (list: move selection, detail: scroll
//! the document). Data is a snapshot fetched over the gateway API; the
//! render path never issues HTTP (same rule as every other widget).
//!
//! Standard [`StatefulWidget`] split, matching the session picker:
//! [`SkillEvolutionWidget`] owns only the chrome (popup geometry and
//! accent), [`SkillEvolutionState`] — held by the app state — is the
//! state.

use gateway::proto::{SkillEvolutionStatus, SkillLibraryDetail, SkillLibraryView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use crate::widgets::popup::{Popup, PopupControls};

const KEYBINDINGS: &str =
    "t trigger · T trigger+auto · a approve · r reject · g refresh · ←/→ panel · Esc close";

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// Which pane owns the vertical keys (j/k, ↑/↓).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PanelFocus {
    /// The skill list — vertical keys move the selection.
    #[default]
    List,
    /// The detail pane — vertical keys scroll the detail document.
    Detail,
}

/// Dashboard state: data snapshots plus selection. Owned by the app
/// state; refreshes land via [`crate::app_event::AppEvent`].
#[derive(Debug, Clone, Default)]
pub struct SkillEvolutionState {
    pub visible: bool,
    pub status: Option<SkillEvolutionStatus>,
    /// The unified library: installed skills (all tiers) plus
    /// proposed-but-not-installed rows, sorted builtin → global →
    /// workspace → proposed.
    pub library: Vec<SkillLibraryView>,
    pub selected: usize,
    /// Which pane the vertical keys act on; ←/→ (h/l) switch it.
    pub focus: PanelFocus,
    /// The selected skill's detail document, with the row name it
    /// belongs to — rendered only while the names match (a slow reply
    /// for a previously selected row is dropped at render time).
    pub detail: Option<(String, SkillLibraryDetail)>,
    /// A failed detail fetch: (row name, error).
    pub detail_error: Option<(String, String)>,
    /// One-line summary of the most recent cycle (from a trigger run
    /// in this TUI session).
    pub last_report: Option<String>,
    /// Last refresh error, rendered in place of the status block.
    pub error: Option<String>,
    /// Render-time selection for the skill list; re-synced from
    /// `selected` on every render so the list scrolls to it.
    list_state: ListState,
    /// Detail-pane scroll offset in lines. Reset whenever the
    /// selection moves (each skill starts at the top); clamped to
    /// the document height at render time.
    detail_scroll: u16,
}

impl SkillEvolutionState {
    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Focus the left (list) pane.
    pub fn focus_list(&mut self) {
        self.focus = PanelFocus::List;
    }

    /// Focus the right (detail) pane.
    pub fn focus_detail(&mut self) {
        self.focus = PanelFocus::Detail;
    }

    /// Scroll the detail pane by `lines` (negative scrolls up).
    /// Clamped to zero here and to the document height at render.
    pub fn scroll_detail(&mut self, lines: i16) {
        self.detail_scroll = if lines < 0 {
            self.detail_scroll.saturating_sub(lines.unsigned_abs())
        } else {
            self.detail_scroll.saturating_add(lines as u16)
        };
    }

    /// Move the selection across all rows — the two-column layout is
    /// a browser: every skill is inspectable. Actions stay gated to
    /// rows with a pending proposal via [`Self::selected_pending`].
    pub fn select_next(&mut self) {
        if self.library.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.library.len();
        self.detail_scroll = 0;
    }

    pub fn select_prev(&mut self) {
        if self.library.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.library.len() - 1
        } else {
            self.selected - 1
        };
        self.detail_scroll = 0;
    }

    /// The skill under the selection, if any.
    pub fn selected_skill(&self) -> Option<&SkillLibraryView> {
        self.library.get(self.selected)
    }

    /// The selected skill **when its proposal attribute is pending** —
    /// the only rows approve/reject act on.
    pub fn selected_pending(&self) -> Option<&SkillLibraryView> {
        self.library
            .get(self.selected)
            .filter(|s| s.proposal_status.as_deref() == Some("pending"))
    }

    /// The detail document for the selected row, when the fetched
    /// detail actually belongs to it (stale replies read as "still
    /// loading").
    fn selected_detail(&self) -> Option<&SkillLibraryDetail> {
        let row = self.selected_skill()?;
        let (name, detail) = self.detail.as_ref()?;
        (name == &row.name).then_some(detail)
    }

    /// The row index to highlight: valid whenever the selection sits
    /// on an existing row.
    fn selected_row(&self) -> Option<usize> {
        (self.selected < self.library.len()).then_some(self.selected)
    }

    /// Point the list state at [`Self::selected_row`] so the list
    /// keeps the selection scrolled into view.
    fn sync_list_state(&mut self) {
        self.list_state.select(self.selected_row());
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// The dashboard as a centered popup: header (configuration +
/// counters + last cycle), the two-pane skill browser, and a
/// key-binding footer. Stateless beyond the chrome — all mutable
/// data lives in [`SkillEvolutionState`].
pub struct SkillEvolutionWidget {
    /// Popup accent color (title/border).
    pub accent: Color,
    /// Popup outer width in cells.
    pub popup_width: u16,
    /// Popup outer height in cells.
    pub popup_height: u16,
}

impl SkillEvolutionWidget {
    pub fn new() -> Self {
        Self {
            accent: Color::Cyan,
            popup_width: 0,
            popup_height: 0,
        }
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn popup_height(mut self, h: u16) -> Self {
        self.popup_height = h;
        self
    }
}

impl Default for SkillEvolutionWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for SkillEvolutionWidget {
    type State = SkillEvolutionState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Skill Evolution ", PopupControls::default())
            .width(self.popup_width)
            .height(self.popup_height)
            .accent(self.accent);
        let inner = popup.render(area, buf);

        // ── error / loading short-circuits ──
        if let Some(error) = &state.error {
            let lines = vec![
                Line::styled(format!("error: {error}"), Style::new().fg(Color::Yellow)),
                Line::styled(
                    "press g to retry, Esc to close",
                    Style::new().fg(Color::DarkGray),
                ),
            ];
            Widget::render(Paragraph::new(lines), inner, buf);
            return;
        }
        let Some(status) = &state.status else {
            Widget::render(
                Paragraph::new(Line::styled(
                    "loading evolution status…",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
                buf,
            );
            return;
        };

        // ── Layout: header (2–4) + browser (rest) + footer (1) ──
        let mut header = vec![config_line(status), counters_line(status)];
        if let Some(report) = &state.last_report {
            header.push(Line::styled(
                format!("last cycle: {report}"),
                Style::new().fg(Color::Cyan),
            ));
        }
        header.push(Line::default());

        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
        Widget::render(Paragraph::new(header), regions[0], buf);
        self.render_browser(regions[1], buf, state);
        Widget::render(
            Paragraph::new(Line::styled(KEYBINDINGS, Style::new().fg(Color::DarkGray))),
            regions[2],
            buf,
        );
    }
}

impl SkillEvolutionWidget {
    /// The skill browser: two columns, matching the picker convention
    /// (see the message picker). Left — scrolling list of name +
    /// tier + proposal-status summaries; right — the selected skill's
    /// detail document.
    fn render_browser(&self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        let dim = Style::new().fg(Color::DarkGray);

        if state.library.is_empty() {
            let lines = vec![
                Line::styled("no skills in the library", dim),
                Line::styled(
                    "install packs (autonomics-skills install) or let the loop distill one",
                    dim,
                ),
            ];
            Widget::render(Paragraph::new(lines), area, buf);
            return;
        }

        // Left: summary list (~40%, clamped for long kebab names).
        // Right: detail pane (rest).
        let list_w = (area.width * 2 / 5).clamp(28, 56);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(list_w), Constraint::Min(20)])
            .split(area);
        self.render_list_column(columns[0], buf, state);
        self.render_detail_column(columns[1], buf, state);
    }

    /// Left column: the summary list behind a right-bordered block
    /// titled with the count. The list state carries the selection;
    /// ratatui scrolls it into view. The title dims when the detail
    /// pane holds focus.
    fn render_list_column(&self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        let focused = state.focus == PanelFocus::List;
        let title_style = if focused {
            Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::new().fg(Color::DarkGray))
            .title(Span::styled(
                format!(" Skills ({}) ", state.library.len()),
                title_style,
            ));
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        let selected = state.selected_row();
        let items: Vec<ListItem<'static>> = state
            .library
            .iter()
            .enumerate()
            .map(|(index, skill)| {
                ListItem::new(skill_row(skill, selected == Some(index), inner.width))
            })
            .collect();
        state.sync_list_state();
        StatefulWidget::render(List::new(items), inner, buf, &mut state.list_state);
    }

    /// Right column: the selected skill's detail as ONE scrollable
    /// document — metadata fields (tier, tags, usage, proposal status
    /// with evidence), then the full SKILL.md body. When this pane
    /// holds focus, j/k/↑/↓ scroll the document (the offset lives in
    /// the state and is clamped to the document height here, where
    /// the height is known).
    fn render_detail_column(&self, area: Rect, buf: &mut Buffer, state: &mut SkillEvolutionState) {
        let focused = state.focus == PanelFocus::Detail;
        let title_style = if focused {
            Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let block = Block::default().title(Span::styled(" Skill ", title_style));
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        let Some(row) = state.selected_skill() else {
            Widget::render(
                Paragraph::new(Line::styled(
                    "select a skill to inspect",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
                buf,
            );
            return;
        };
        // A stale or failed fetch for a previously selected row reads
        // as transient state for the current one.
        if let Some((failed_for, error)) = &state.detail_error
            && failed_for == &row.name
        {
            Widget::render(
                Paragraph::new(Line::styled(
                    format!("detail unavailable: {error}"),
                    Style::new().fg(Color::Yellow),
                )),
                inner,
                buf,
            );
            return;
        }
        let Some(detail) = state.selected_detail() else {
            Widget::render(
                Paragraph::new(Line::styled(
                    "loading detail…",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
                buf,
            );
            return;
        };

        let lines = detail_document(detail);
        // Clamp the scroll so the last line stays reachable but the
        // document never scrolls past its end.
        let height = inner.height as usize;
        let total: usize = lines
            .iter()
            .map(|line| line_rows(line, inner.width as usize))
            .sum();
        let max_scroll = total.saturating_sub(height) as u16;
        state.detail_scroll = state.detail_scroll.min(max_scroll);

        Widget::render(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((state.detail_scroll, 0)),
            inner,
            buf,
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Row / line builders
// ═══════════════════════════════════════════════════════════════════════

/// Service configuration + generation line.
fn config_line(status: &SkillEvolutionStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let service = if status.service_enabled {
        Span::styled("on".to_string(), Style::new().fg(Color::Cyan))
    } else {
        Span::styled("off".to_string(), Style::new().fg(Color::Yellow))
    };
    let gate = if status.auto_approve {
        Span::styled("auto-approve".to_string(), Style::new().fg(Color::Yellow))
    } else {
        Span::styled("propose-only".to_string(), Style::new().fg(Color::Cyan))
    };
    let sweep = status
        .sweep_interval_secs
        .map(|s| format!("sweep {}s", s))
        .unwrap_or_else(|| "no sweep".to_string());
    Line::from(vec![
        Span::styled("service ", dim),
        service,
        Span::styled("  ·  gate ", dim),
        gate,
        Span::styled(format!(" (cap {})", status.max_approvals_per_cycle), dim),
        Span::styled("  ·  ", dim),
        Span::styled(sweep, Style::new()),
        Span::styled(
            format!("  ·  generation {}", status.generation),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Observation / skill / proposal counters line.
fn counters_line(status: &SkillEvolutionStatus) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new();
    Line::from(vec![
        Span::styled("observations ", dim),
        Span::styled(format!("{}", status.observations), normal),
        Span::styled("   skills ", dim),
        Span::styled(
            format!(
                "{} auto / {} global / {} workspace / {} builtin",
                status.skills_auto,
                status.skills_global,
                status.skills_workspace,
                status.skills_builtin
            ),
            normal,
        ),
        Span::styled("   proposals ", dim),
        Span::styled(
            format!(
                "{} pending · {} approved · {} rejected",
                status.proposals_pending, status.proposals_approved, status.proposals_rejected
            ),
            normal,
        ),
    ])
}

/// One summary row for the left column: marker + name + tier +
/// proposal status, truncated to the column width — the right pane
/// carries the full detail, the list row only has to be scannable.
fn skill_row(skill: &SkillLibraryView, selected: bool, width: u16) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new();
    let bold = Style::new().add_modifier(Modifier::BOLD);

    let status_style = match skill.proposal_status.as_deref() {
        Some("pending") => Style::new().fg(Color::Yellow),
        Some("approved") => Style::new().fg(Color::Green),
        _ => Style::new().fg(Color::DarkGray),
    };
    let marker = if selected { "▶ " } else { "  " };
    // Tier glyph: one scannable character per tier.
    let tier_tag = match skill.tier.as_str() {
        "builtin" => "b",
        "workspace" => "w",
        "proposed" => "p",
        _ => "g",
    };

    // name budget: width − marker(2) − tier(3) − status tag(~12)
    let name_budget = (width as usize).saturating_sub(17).max(8);
    let mut spans = vec![
        Span::styled(marker, Style::new().fg(Color::Cyan)),
        Span::styled(
            truncate(&skill.name, name_budget),
            if selected { bold } else { normal },
        ),
        Span::styled(format!(" [{tier_tag}]"), dim),
    ];
    if let Some(status) = &skill.proposal_status {
        spans.push(Span::styled(
            format!(" {status}"),
            if selected { status_style } else { dim },
        ));
    }
    Line::from(spans)
}

/// The detail document: metadata fields with dim labels, then the
/// SKILL.md body under a section header. Assembled as one `Vec<Line>`
/// so it scrolls as a single document.
fn detail_document(detail: &SkillLibraryDetail) -> Vec<Line<'static>> {
    let label = Style::new().fg(Color::DarkGray);
    let value = Style::new();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(Color::DarkGray);

    let field = |name: &str, val: String, style: Style| {
        Line::from(vec![
            Span::styled(format!("{name:<10}"), label),
            Span::styled(val, style),
        ])
    };

    let mut lines = vec![
        field("Name", detail.name.clone(), bold),
        field(
            "Tier",
            if detail.installed {
                format!("{} · installed", detail.tier)
            } else {
                format!("{} — not installed yet", detail.tier)
            },
            if detail.installed {
                value
            } else {
                Style::new().fg(Color::Yellow)
            },
        ),
    ];
    if !detail.tags.is_empty() {
        lines.push(field("Tags", detail.tags.join(", "), dim));
    }
    // Usage telemetry — the fitness signal.
    let usage = if detail.usage_gets == 0
        && detail.usage_search_hits == 0
        && detail.usage_runs == 0
        && detail.usage_evals == 0
    {
        "never used".to_string()
    } else {
        format!(
            "{} get · {} search · {} run · {} eval · last {}",
            detail.usage_gets,
            detail.usage_search_hits,
            detail.usage_runs,
            detail.usage_evals,
            if detail.usage_last_used == 0 {
                "—".to_string()
            } else {
                format_created(detail.usage_last_used)
            }
        )
    };
    lines.push(field("Usage", usage, value));
    // Bundles, installed rows only.
    if detail.installed && (!detail.workflows.is_empty() || !detail.evals.is_empty()) {
        let mut bundles = Vec::new();
        if !detail.workflows.is_empty() {
            bundles.push(format!("{} workflow(s)", detail.workflows.len()));
        }
        if !detail.evals.is_empty() {
            bundles.push(format!("{} eval(s)", detail.evals.len()));
        }
        lines.push(field("Bundles", bundles.join(" · "), dim));
    }
    // The proposal attribute: status line + a dim continuation with
    // author, evidence size, and the rationale.
    match &detail.proposal {
        Some(proposal) => {
            let status_style = match proposal.status.as_str() {
                "pending" => Style::new().fg(Color::Yellow),
                "approved" => Style::new().fg(Color::Green),
                _ => Style::new().fg(Color::DarkGray),
            };
            let status_note = match proposal.status.as_str() {
                "pending" => " — awaiting review",
                "approved" => " — promoted to the library",
                _ => " — cluster consumed, will not re-propose",
            };
            lines.push(field(
                "Proposal",
                format!("{}{status_note}", proposal.status),
                status_style,
            ));
            let mut meta = format!(
                "by {} · {} obs evidence",
                proposal.authored_by, proposal.observation_count
            );
            if !proposal.rationale.is_empty() {
                meta.push_str(&format!(" — {}", proposal.rationale));
            }
            lines.push(Line::from(vec![
                Span::styled("           ", label),
                Span::styled(meta, dim),
            ]));
        }
        None => lines.push(field("Proposal", "— (outside the pipeline)".into(), dim)),
    }
    // Body section: the full SKILL.md content.
    lines.push(Line::styled(
        "Body — SKILL.md",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    if detail.body.trim().is_empty() {
        lines.push(Line::styled("(empty body)", dim));
    } else {
        for source in detail.body.lines() {
            lines.push(Line::from(source.to_string()));
        }
    }
    lines
}

/// Format a unix-seconds timestamp as a relative age (same convention
/// as the session picker's timestamps).
fn format_created(secs: i64) -> String {
    let Some(created) = chrono::DateTime::from_timestamp(secs, 0) else {
        return "unknown".to_string();
    };
    let delta = chrono::Utc::now().timestamp() - secs;
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 7 * 86_400 {
        format!("{}d ago", delta / 86_400)
    } else {
        created.format("%Y-%m-%d %H:%M").to_string()
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// A rendered line's row count when word-wrapped at `width` — the
/// same greedy algorithm ratatui's `Wrap` applies, so the scroll
/// clamp matches what actually renders.
fn line_rows(line: &Line<'_>, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
    if text.is_empty() {
        return 1;
    }
    let mut rows = 1usize;
    let mut current = 0usize;
    for word in text.split(' ') {
        let word_len = word.chars().count();
        // A word longer than the pane wraps mid-word across rows.
        let overflow = word_len.saturating_sub(width);
        if overflow > 0 {
            rows += word_len.div_ceil(width) - 1;
            current = word_len % width;
            continue;
        }
        if current == 0 {
            current = word_len;
        } else if current + 1 + word_len <= width {
            current += 1 + word_len;
        } else {
            rows += 1;
            current = word_len;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, tier: &str, proposal_status: Option<&str>) -> SkillLibraryView {
        SkillLibraryView {
            name: name.into(),
            tier: tier.into(),
            tags: vec!["auto".into()],
            description: "does the thing".into(),
            installed: proposal_status.is_none() || tier != "proposed",
            proposal_status: proposal_status.map(Into::into),
            proposal_update: false,
            usage_gets: 1,
            usage_search_hits: 0,
            usage_runs: 0,
            usage_evals: 0,
            usage_last_used: 0,
        }
    }

    fn detail_for(name: &str) -> SkillLibraryDetail {
        SkillLibraryDetail {
            name: name.into(),
            tier: "global".into(),
            tags: vec!["auto".into()],
            description: "does the thing".into(),
            installed: true,
            body: "# Guide\n\nStep one.\n".into(),
            workflows: vec![],
            evals: vec![],
            proposal: None,
            evidence_count: 0,
            usage_gets: 0,
            usage_search_hits: 0,
            usage_runs: 0,
            usage_evals: 0,
            usage_last_used: 0,
        }
    }

    fn draw(state: &mut SkillEvolutionState, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::StatefulWidget as _;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                SkillEvolutionWidget::new()
                    .popup_width(frame.area().width * 9 / 10)
                    .popup_height((frame.area().height * 3 / 4).max(12))
                    .render(frame.area(), frame.buffer_mut(), state)
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn selection_moves_across_all_rows_with_wrap() {
        let mut state = SkillEvolutionState {
            library: vec![
                skill("a-pending", "global", Some("pending")),
                skill("b-approved", "global", Some("approved")),
                skill("c-plain", "builtin", None),
                skill("d-rejected", "global", Some("rejected")),
            ],
            ..Default::default()
        };
        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 1, "visits history rows too");
        state.select_next();
        state.select_next();
        state.select_next();
        assert_eq!(state.selected, 0, "wraps forward");
        state.select_prev();
        assert_eq!(state.selected, 3, "wraps backwards");
        // Actions stay gated to pending rows regardless of where the
        // browser selection sits.
        state.selected = 1;
        assert!(
            state.selected_pending().is_none(),
            "approved row is not an action target"
        );
        assert_eq!(
            state.selected_skill().map(|s| s.name.clone()),
            Some("b-approved".into()),
            "but the detail pane still shows it"
        );
    }

    #[test]
    fn no_pending_rows_means_no_selection_action() {
        let mut state = SkillEvolutionState {
            library: vec![skill("only-history", "builtin", None)],
            selected: 0,
            ..Default::default()
        };
        state.select_next();
        assert!(state.selected_pending().is_none());
    }

    #[test]
    fn render_shows_library_and_detail_document() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(SkillEvolutionStatus {
                service_enabled: true,
                generation: 7,
                auto_approve: false,
                max_approvals_per_cycle: 5,
                sweep_interval_secs: Some(21600),
                observations: 12,
                skills_workspace: 0,
                skills_global: 1,
                skills_builtin: 1,
                skills_auto: 1,
                proposals_pending: 1,
                proposals_approved: 0,
                proposals_rejected: 0,
            }),
            library: vec![skill("sql-syntax", "global", Some("pending"))],
            detail: Some((
                "sql-syntax".into(),
                SkillLibraryDetail {
                    proposal: Some(gateway::proto::SkillProposalView {
                        name: "sql-syntax".into(),
                        status: "pending".into(),
                        authored_by: "distiller".into(),
                        update: false,
                        rationale: "three observations".into(),
                        cluster_hash: "c-abc".into(),
                        observation_count: 3,
                        created_at: 1,
                    }),
                    evidence_count: 3,
                    ..detail_for("sql-syntax")
                },
            )),
            last_report: Some("1 written".into()),
            ..Default::default()
        };
        // The full document (metadata + proposal + body) needs a tall
        // popup to fit unscrolled; shorter terminals scroll it.
        let text = draw(&mut state, 100, 32);
        assert!(text.contains("generation 7"), "{text}");
        assert!(text.contains("Skills (1)"));
        assert!(text.contains("sql-syntax"));
        assert!(text.contains("pending"), "proposal status on the row");
        // Detail document: metadata + usage + proposal + body.
        assert!(text.contains("Tier"));
        assert!(text.contains("installed"));
        assert!(
            text.contains("never used"),
            "zero telemetry reads as unused"
        );
        assert!(text.contains("awaiting review"));
        // The rationale may word-wrap mid-phrase, so assert on a
        // fragment that fits one rendered row.
        assert!(text.contains("obs evidence"));
        assert!(text.contains("Body — SKILL.md"));
        assert!(text.contains("Step one."));
    }

    #[test]
    fn stale_detail_for_another_row_shows_loading() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            library: vec![skill("a", "global", None), skill("b", "global", None)],
            selected: 1,
            // Detail fetched for row "a" while the selection has since
            // moved to "b" — must not render as b's detail.
            detail: Some(("a".into(), detail_for("a"))),
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("loading detail"), "{text}");
        assert!(!text.contains("Step one"), "stale body must not show");
    }

    #[test]
    fn detail_fetch_failure_renders_the_error() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            library: vec![skill("gone", "global", None)],
            detail_error: Some(("gone".into(), "404 no skill".into())),
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("detail unavailable"), "{text}");
        assert!(text.contains("404"));
    }

    fn dummy_status() -> SkillEvolutionStatus {
        SkillEvolutionStatus {
            service_enabled: true,
            generation: 1,
            auto_approve: false,
            max_approvals_per_cycle: 5,
            sweep_interval_secs: None,
            observations: 0,
            skills_workspace: 0,
            skills_global: 0,
            skills_builtin: 0,
            skills_auto: 0,
            proposals_pending: 0,
            proposals_approved: 0,
            proposals_rejected: 0,
        }
    }

    #[test]
    fn render_error_state_without_panicking() {
        let mut state = SkillEvolutionState {
            visible: true,
            error: Some("daemon unreachable".into()),
            ..Default::default()
        };
        let text = draw(&mut state, 80, 10);
        assert!(text.contains("daemon unreachable"));
    }

    #[test]
    fn hidden_state_renders_nothing() {
        let mut state = SkillEvolutionState {
            visible: false,
            status: None,
            ..Default::default()
        };
        let text = draw(&mut state, 80, 10);
        assert!(
            text.trim().is_empty(),
            "expected a blank frame, got {text:?}"
        );
    }

    #[test]
    fn selection_scrolls_into_view_when_queue_overflows() {
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            library: (0..12)
                .map(|i| skill(&format!("skill-{i:02}"), "global", None))
                .collect(),
            selected: 11,
            ..Default::default()
        };
        let text = draw(&mut state, 80, 24);
        assert!(text.contains("skill-11"), "selected row must be visible");
        assert!(
            !text.contains("skill-00"),
            "first row must have scrolled out: {text}"
        );
    }

    #[test]
    fn focus_switches_between_panes_and_vertical_keys_follow() {
        let mut state = SkillEvolutionState {
            library: vec![skill("a", "global", None)],
            ..Default::default()
        };
        assert_eq!(state.focus, PanelFocus::List, "starts on the list");
        state.focus_detail();
        assert_eq!(state.focus, PanelFocus::Detail);
        state.scroll_detail(3);
        state.scroll_detail(-1);
        state.focus_list();
        assert_eq!(state.focus, PanelFocus::List);
    }

    #[test]
    fn detail_scroll_resets_on_selection_change_and_clamps_at_render() {
        let long_body = "START\n".to_string() + &"filler line\n".repeat(40) + "END\n";
        let mut state = SkillEvolutionState {
            visible: true,
            status: Some(dummy_status()),
            library: vec![
                skill("long-body", "global", None),
                skill("b", "global", None),
            ],
            detail: Some((
                "long-body".into(),
                SkillLibraryDetail {
                    body: long_body,
                    ..detail_for("long-body")
                },
            )),
            ..Default::default()
        };
        state.focus_detail();
        state.scroll_detail(500);
        // Render clamps the offset to the document height — START
        // scrolls out, END (the last line) stays on screen.
        let text = draw(&mut state, 100, 24);
        assert!(
            text.contains("END"),
            "last line reachable after clamp: {text}"
        );
        assert!(
            !text.contains("START"),
            "top must have scrolled away: {text}"
        );

        // Moving the selection restarts the next detail from the top.
        state.select_next();
        assert_eq!(state.detail_scroll, 0, "selection change resets scroll");
    }
}
