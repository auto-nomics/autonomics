//! Two-column skill library browser.

use gateway::proto::{SkillLibraryDetail, SkillLibraryView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

/// Which pane owns the vertical keys (j/k, ↑/↓).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PanelFocus {
    /// The skill list — vertical keys move the selection.
    #[default]
    List,
    /// The detail pane — vertical keys scroll the detail document.
    Detail,
}

/// Library snapshot, selection, and scrolling state for the Skills tab.
#[derive(Debug, Clone, Default)]
pub struct SkillBrowserState {
    pub error: Option<String>,
    /// The unified library: installed skills (all tiers) plus
    /// proposed-but-not-installed rows, sorted builtin → global →
    /// workspace → proposed.
    pub library: Vec<SkillLibraryView>,
    pub selected: usize,
    /// Which pane the vertical keys act on; ←/→ (h/l) switch it.
    pub focus: PanelFocus,
    /// The selected skill's detail document, with the row name it
    /// belongs to — rendered only while the names match. Event handling
    /// also ignores late replies for a previously selected row.
    pub detail: Option<(String, SkillLibraryDetail)>,
    /// A failed detail fetch: (row name, error).
    pub detail_error: Option<(String, String)>,
    /// Render-time selection for the skill list; re-synced from
    /// `selected` on every render so the list scrolls to it.
    pub(super) list_state: ListState,
    /// Detail-pane scroll offset in lines. Reset whenever the
    /// selection moves (each skill starts at the top); clamped to
    /// the document height at render time.
    pub(super) detail_scroll: u16,
}

impl SkillBrowserState {
    /// Ignore late replies without overwriting the current row's document.
    pub fn apply_detail(&mut self, name: String, result: Result<SkillLibraryDetail, String>) {
        if self.selected_skill().is_none_or(|row| row.name != name) {
            return;
        }
        match result {
            Ok(detail) => {
                self.detail = Some((name, detail));
                self.detail_error = None;
            }
            Err(error) => {
                self.detail = None;
                self.detail_error = Some((name, error));
            }
        }
    }
    /// Keep the selected skill across refreshes, falling back to a pending row.
    pub fn set_library(&mut self, library: Vec<SkillLibraryView>) {
        let name = self.selected_skill().map(|row| row.name.clone());
        self.selected = name
            .as_ref()
            .and_then(|name| library.iter().position(|row| &row.name == name))
            .or_else(|| {
                library
                    .iter()
                    .position(|row| row.proposal_status.as_deref() == Some("pending"))
            })
            .unwrap_or(0);
        if library.get(self.selected).map(|row| &row.name) != name.as_ref() {
            self.detail_scroll = 0;
        }
        self.library = library;
        self.detail = None;
        self.detail_error = None;
        self.error = None;
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

pub struct SkillBrowserWidget {
    pub accent: Color,
}
impl StatefulWidget for SkillBrowserWidget {
    type State = SkillBrowserState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        self.render_browser(area, buf, state);
    }
}
impl SkillBrowserWidget {
    /// The skill browser: two columns, matching the picker convention
    /// (see the message picker). Left — scrolling list of name +
    /// tier + proposal-status summaries; right — the selected skill's
    /// detail document.
    fn render_browser(&self, area: Rect, buf: &mut Buffer, state: &mut SkillBrowserState) {
        let dim = Style::new().fg(Color::DarkGray);

        if let Some(error) = &state.error {
            Widget::render(
                Paragraph::new(format!("skills unavailable: {error}; press g to retry"))
                    .style(Style::new().fg(Color::Yellow))
                    .wrap(Wrap { trim: false }),
                area,
                buf,
            );
            return;
        }

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
    fn render_list_column(&self, area: Rect, buf: &mut Buffer, state: &mut SkillBrowserState) {
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
    fn render_detail_column(&self, area: Rect, buf: &mut Buffer, state: &mut SkillBrowserState) {
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

        let paragraph = Paragraph::new(detail_document(detail)).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(inner.width)
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        state.detail_scroll = state.detail_scroll.min(max_scroll);
        Widget::render(paragraph.scroll((state.detail_scroll, 0)), inner, buf);
    }
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
        // An update proposal revises an installed skill rather than
        // adding a new one — the ↑ marks it in the narrow list column
        // (the detail pane spells out what approving it does).
        let status_text = if skill.proposal_update {
            format!(" {status}↑")
        } else {
            format!(" {status}")
        };
        spans.push(Span::styled(
            status_text,
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
    if !detail.description.is_empty() {
        lines.push(field("Summary", detail.description.clone(), value));
    }
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
            // Update proposals revise an installed skill (the previous
            // version archives to .history/ on approve); creates add a
            // new one. The status line says which.
            let kind_note = if proposal.update {
                "update of installed skill — previous version archives on approve"
            } else {
                ""
            };
            let status_note = match proposal.status.as_str() {
                "pending" => " — awaiting review",
                "approved" => " — promoted to the library",
                _ => " — cluster consumed, will not re-propose",
            };
            let status_line = if proposal.update {
                format!("update {status_note} · {kind_note}")
            } else {
                format!("{}{status_note}", proposal.status)
            };
            lines.push(field("Proposal", status_line, status_style));
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
pub(super) fn format_created(secs: i64) -> String {
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

pub(super) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
