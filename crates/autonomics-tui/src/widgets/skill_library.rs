//! Skill library dashboard: a centered popup browser over the
//! installed skill tiers. Two columns — the scannable list and the
//! selected skill's full document.

use gateway::proto::{SkillLibraryDetail, SkillLibraryView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
};

use crate::widgets::popup::{Popup, PopupControls};

/// Which pane owns the vertical keys (j/k, ↑/↓).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PanelFocus {
    /// The skill list — vertical keys move the selection.
    #[default]
    List,
    /// The detail pane — vertical keys scroll the detail document.
    Detail,
}

/// Dashboard state: visibility plus the browser snapshot.
#[derive(Debug, Clone, Default)]
pub struct SkillLibraryState {
    pub visible: bool,
    pub browser: SkillBrowserState,
}

impl SkillLibraryState {
    pub fn open(&mut self) {
        self.visible = true;
    }
    pub fn close(&mut self) {
        self.visible = false;
    }
    /// True when the skill selection changed and needs a detail fetch.
    pub fn navigate(&mut self, down: bool) -> bool {
        if self.browser.focus == PanelFocus::Detail {
            self.browser.scroll_detail(if down { 1 } else { -1 });
            false
        } else {
            let previous = self.browser.selected;
            if down {
                self.browser.select_next();
            } else {
                self.browser.select_prev();
            }
            previous != self.browser.selected
        }
    }
}

/// Library snapshot, selection, and scrolling state for the browser.
#[derive(Debug, Clone, Default)]
pub struct SkillBrowserState {
    pub error: Option<String>,
    /// The installed library, sorted builtin → global → workspace.
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
    /// Keep the selected skill across refreshes.
    pub fn set_library(&mut self, library: Vec<SkillLibraryView>) {
        let name = self.selected_skill().map(|row| row.name.clone());
        self.selected = name
            .as_ref()
            .and_then(|name| library.iter().position(|row| &row.name == name))
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
    /// a browser: every skill is inspectable.
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

/// The dashboard as a centered popup. Stateless beyond the chrome —
/// all mutable data lives in [`SkillLibraryState`].
pub struct SkillLibraryWidget {
    /// Popup accent color (title/border).
    pub accent: Color,
    /// Popup outer width in cells.
    pub popup_width: u16,
    /// Popup outer height in cells.
    pub popup_height: u16,
}

impl SkillLibraryWidget {
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

impl Default for SkillLibraryWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for SkillLibraryWidget {
    type State = SkillLibraryState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SkillLibraryState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Skill Library ", PopupControls::default())
            .width(self.popup_width)
            .height(self.popup_height)
            .accent(self.accent);
        let inner = popup.render(area, buf);

        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        StatefulWidget::render(
            SkillBrowserWidget {
                accent: self.accent,
            },
            regions[0],
            buf,
            &mut state.browser,
        );
        Widget::render(
            Paragraph::new(Line::raw(
                "←→ panel · ↑↓ move/scroll · g refresh · Esc close",
            ))
            .style(Style::new().fg(Color::DarkGray)),
            regions[1],
            buf,
        );
    }
}

pub struct SkillBrowserWidget {
    pub accent: Color,
}
impl StatefulWidget for SkillBrowserWidget {
    type State = SkillBrowserState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SkillBrowserState) {
        self.render_browser(area, buf, state);
    }
}
impl SkillBrowserWidget {
    /// The skill browser: two columns, matching the picker convention
    /// (see the message picker). Left — scrolling list of name +
    /// tier summaries; right — the selected skill's detail document.
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
                    "install packs with `autonomics-skills install <source>`",
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
    /// document — metadata fields (tier, tags, usage), then the full
    /// SKILL.md body. When this pane holds focus, j/k/↑/↓ scroll the
    /// document (the offset lives in the state and is clamped to the
    /// document height here, where the height is known).
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

/// One summary row for the left column: marker + name + tier,
/// truncated to the column width — the right pane carries the full
/// detail, the list row only has to be scannable.
fn skill_row(skill: &SkillLibraryView, selected: bool, width: u16) -> Line<'static> {
    let dim = Style::new().fg(Color::DarkGray);
    let normal = Style::new();
    let bold = Style::new().add_modifier(Modifier::BOLD);

    let marker = if selected { "▶ " } else { "  " };
    // Tier glyph: one scannable character per tier.
    let tier_tag = match skill.tier.as_str() {
        "builtin" => "b",
        "workspace" => "w",
        _ => "g",
    };

    // name budget: width − marker(2) − tier(3)
    let name_budget = (width as usize).saturating_sub(5).max(8);
    Line::from(vec![
        Span::styled(marker, Style::new().fg(Color::Cyan)),
        Span::styled(
            truncate(&skill.name, name_budget),
            if selected { bold } else { normal },
        ),
        Span::styled(format!(" [{tier_tag}]"), dim),
    ])
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
        field("Tier", detail.tier.clone(), value),
    ];
    if !detail.description.is_empty() {
        lines.push(field("Summary", detail.description.clone(), value));
    }
    if !detail.tags.is_empty() {
        lines.push(field("Tags", detail.tags.join(", "), dim));
    }
    // Usage telemetry.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn skill(name: &str, tier: &str) -> SkillLibraryView {
        SkillLibraryView {
            name: name.into(),
            tier: tier.into(),
            tags: vec!["meta".into()],
            description: "does the thing".into(),
            installed: true,
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
            tags: vec!["meta".into()],
            description: "does the thing".into(),
            installed: true,
            body: "# Guide\n\nStep one.\n".into(),
            workflows: vec![],
            evals: vec![],
            usage_gets: 0,
            usage_search_hits: 0,
            usage_runs: 0,
            usage_evals: 0,
            usage_last_used: 0,
        }
    }

    fn draw(state: &mut SkillLibraryState, width: u16, height: u16) -> String {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                SkillLibraryWidget::new()
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
        let mut state = SkillLibraryState {
            browser: SkillBrowserState {
                library: vec![
                    skill("a", "global"),
                    skill("b", "builtin"),
                    skill("c", "workspace"),
                    skill("d", "global"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(state.browser.selected, 0);
        state.browser.select_next();
        assert_eq!(state.browser.selected, 1, "visits every row");
        state.browser.select_next();
        state.browser.select_next();
        state.browser.select_next();
        assert_eq!(state.browser.selected, 0, "wraps forward");
        state.browser.select_prev();
        assert_eq!(state.browser.selected, 3, "wraps backwards");
        assert_eq!(
            state.browser.selected_skill().map(|s| s.name.clone()),
            Some("d".into())
        );
    }

    #[test]
    fn render_shows_library_and_detail_document() {
        let mut state = SkillLibraryState {
            visible: true,
            browser: SkillBrowserState {
                library: vec![skill("sql-syntax", "global")],
                detail: Some(("sql-syntax".into(), detail_for("sql-syntax"))),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 100, 32);
        assert!(text.contains("Skills (1)"), "{text}");
        assert!(text.contains("sql-syntax"), "{text}");
        // Detail document: metadata + usage + body.
        assert!(text.contains("Tier"));
        assert!(
            text.contains("never used"),
            "zero telemetry reads as unused"
        );
        assert!(text.contains("Body — SKILL.md"));
        assert!(text.contains("Step one."));
    }

    #[test]
    fn stale_detail_for_another_row_shows_loading() {
        let mut state = SkillLibraryState {
            visible: true,
            browser: SkillBrowserState {
                library: vec![skill("a", "global"), skill("b", "global")],
                selected: 1,
                // Detail fetched for row "a" while the selection has since
                // moved to "b" — must not render as b's detail.
                detail: Some(("a".into(), detail_for("a"))),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("loading detail"), "{text}");
        assert!(!text.contains("Step one"), "stale body must not show");
    }

    #[test]
    fn detail_fetch_failure_renders_the_error() {
        let mut state = SkillLibraryState {
            visible: true,
            browser: SkillBrowserState {
                library: vec![skill("gone", "global")],
                detail_error: Some(("gone".into(), "404 no skill".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 100, 24);
        assert!(text.contains("detail unavailable"), "{text}");
        assert!(text.contains("404"));
    }

    #[test]
    fn library_error_state_renders_without_panicking() {
        let mut state = SkillLibraryState {
            visible: true,
            browser: SkillBrowserState {
                error: Some("daemon unreachable".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 80, 10);
        assert!(text.contains("daemon unreachable"));
    }

    #[test]
    fn empty_library_renders_the_install_hint() {
        let mut state = SkillLibraryState {
            visible: true,
            ..Default::default()
        };
        let text = draw(&mut state, 80, 12);
        assert!(text.contains("no skills in the library"), "{text}");
        assert!(text.contains("autonomics-skills install"));
    }

    #[test]
    fn hidden_state_renders_nothing() {
        let mut state = SkillLibraryState {
            visible: false,
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
        // 80×24 → popup 18 rows → ~15 visible list rows; 20 skills
        // force the list to scroll.
        let mut state = SkillLibraryState {
            visible: true,
            browser: SkillBrowserState {
                library: (0..20)
                    .map(|i| skill(&format!("skill-{i:02}"), "global"))
                    .collect(),
                selected: 19,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&mut state, 80, 24);
        assert!(text.contains("skill-19"), "selected row must be visible");
        assert!(
            !text.contains("skill-00"),
            "first row must have scrolled out: {text}"
        );
    }

    #[test]
    fn focus_switches_between_panes_and_vertical_keys_follow() {
        let mut state = SkillLibraryState {
            browser: SkillBrowserState {
                library: vec![skill("a", "global")],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(state.browser.focus, PanelFocus::List, "starts on the list");
        state.browser.focus_detail();
        assert_eq!(state.browser.focus, PanelFocus::Detail);
        state.browser.scroll_detail(3);
        state.browser.scroll_detail(-1);
        state.browser.focus_list();
        assert_eq!(state.browser.focus, PanelFocus::List);
    }

    #[test]
    fn navigate_reports_selection_changes() {
        let mut state = SkillLibraryState {
            browser: SkillBrowserState {
                library: vec![skill("a", "global"), skill("b", "global")],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(state.navigate(true), "selection moved → detail fetch");
        state.browser.focus_detail();
        assert!(!state.navigate(true), "scrolling the detail pane");
    }
}
