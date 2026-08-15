//! Dedicated KMS tree browser.
//!
//! This ports the domain-specific panels from the original dendrite `kms_tui`
//! into the autonomics TUI: tree navigation, knowledge/entity inspection, and
//! diagnostics. It deliberately reuses the same shared `agent.db` connection
//! as the runtime instead of opening a second KMS database.

use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout, Write, stdout};
use std::sync::Arc;

use agentik_core::TursoAgentStorage;
use kms::{Diagnostic, Entity, Index, KmsService, Knowledge, Severity, TargetType};
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Terminal,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use uuid::Uuid;

const HALF_PAGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Tree,
    Details,
    Diagnostics,
}

impl Panel {
    fn next(self) -> Self {
        match self {
            Self::Tree => Self::Details,
            Self::Details => Self::Diagnostics,
            Self::Diagnostics => Self::Tree,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailsTab {
    Knowledge,
    Entity,
}

#[derive(Debug, Clone)]
struct TreeNode {
    value: Index,
    children: Vec<TreeNode>,
}

impl TreeNode {
    fn flatten<'a>(
        &'a self,
        expanded: &HashSet<Uuid>,
        indent: usize,
        output: &mut Vec<(&'a TreeNode, usize)>,
    ) {
        output.push((self, indent));
        if expanded.contains(&self.value.id) {
            for child in &self.children {
                child.flatten(expanded, indent + 1, output);
            }
        }
    }
}

#[derive(Default)]
struct Theme {
    focus: Color,
    muted: Color,
    selected: Color,
    group: Color,
    knowledge: Color,
    error: Color,
    warning: Color,
}

impl Theme {
    fn new() -> Self {
        Self {
            focus: Color::Cyan,
            muted: Color::DarkGray,
            selected: Color::LightYellow,
            group: Color::Blue,
            knowledge: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
        }
    }
}

pub struct KmsTui {
    service: Arc<KmsService>,
    root: Option<TreeNode>,
    visible: Vec<(Uuid, usize)>,
    by_id: HashMap<Uuid, Index>,
    knowledge_by_id: HashMap<Uuid, Knowledge>,
    entity_by_id: HashMap<Uuid, Entity>,
    diagnostics: Vec<Diagnostic>,
    expanded: HashSet<Uuid>,
    selected: usize,
    tree_state: ListState,
    details_tab: DetailsTab,
    details_scroll: usize,
    diagnostics_scroll: usize,
    panel: Panel,
    status: String,
    theme: Theme,
}

impl KmsTui {
    pub async fn new(agent_db: &std::path::Path) -> Result<Self, String> {
        let storage = TursoAgentStorage::open(agent_db)
            .await
            .map_err(|error| error.to_string())?;
        let kms_storage = kms::Storage::from_shared_connection(storage.shared_connection()).await?;
        let service = Arc::new(KmsService::from_storage(kms_storage).await?);
        let mut app = Self {
            service,
            root: None,
            visible: Vec::new(),
            by_id: HashMap::new(),
            knowledge_by_id: HashMap::new(),
            entity_by_id: HashMap::new(),
            diagnostics: Vec::new(),
            expanded: HashSet::new(),
            selected: 0,
            tree_state: ListState::default(),
            details_tab: DetailsTab::Knowledge,
            details_scroll: 0,
            diagnostics_scroll: 0,
            panel: Panel::Tree,
            status: String::new(),
            theme: Theme::new(),
        };
        app.reload().await?;
        let root_id = app.service.find_root().await?.id;
        app.expanded.insert(root_id);
        let root = app.root.clone().expect("KMS root loaded");
        app.rebuild_visible(&root);
        Ok(app)
    }

    async fn reload(&mut self) -> Result<(), String> {
        let root_value = self.service.find_root().await?;
        let root_id = root_value.id;
        let root = self.load_tree(root_value).await?;
        self.by_id.clear();
        index_tree(&root, &mut self.by_id);

        let all_knowledge = self.service.get_subtree_knowledge(root_id).await?;
        self.knowledge_by_id = all_knowledge
            .into_iter()
            .map(|knowledge| (knowledge.id, knowledge))
            .collect();

        let all_entities = self.service.list_entities(kms::EntityFilter::All).await?;
        self.entity_by_id = all_entities
            .into_iter()
            .map(|entity| (entity.id, entity))
            .collect();

        self.diagnostics = self.service.diagnose().await?;
        self.rebuild_visible(&root);
        self.root = Some(root);
        self.status = format!(
            "{} nodes | {} knowledge | {} entities | {} diagnostics",
            self.by_id.len(),
            self.knowledge_by_id.len(),
            self.entity_by_id.len(),
            self.diagnostics.len()
        );
        Ok(())
    }

    async fn load_tree(&self, value: Index) -> Result<TreeNode, String> {
        let children = self.service.get_children(Some(value.id)).await?;
        let mut loaded = Vec::with_capacity(children.len());
        for child in children {
            loaded.push(Box::pin(self.load_tree(child)).await?);
        }
        Ok(TreeNode {
            value,
            children: loaded,
        })
    }

    fn rebuild_visible(&mut self, root: &TreeNode) {
        let mut flat = Vec::new();
        root.flatten(&self.expanded, 0, &mut flat);
        self.visible = flat
            .into_iter()
            .map(|(node, indent)| (node.value.id, indent))
            .collect();
        if self.selected >= self.visible.len() {
            self.selected = self.visible.len().saturating_sub(1);
        }
        self.tree_state.select(Some(self.selected));
    }

    pub async fn run(&mut self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(stdout(), crossterm::terminal::EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        let result = self.run_loop(&mut terminal).await;
        crossterm::execute!(stdout(), crossterm::terminal::LeaveAlternateScreen)?;
        crossterm::terminal::disable_raw_mode()?;
        stdout().flush()?;
        result
    }

    async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> io::Result<()> {
        loop {
            terminal.draw(|frame| self.render(frame))?;
            if !crossterm::event::poll(std::time::Duration::from_millis(250))? {
                continue;
            }
            let event = crossterm::event::read()?;
            if let crossterm::event::Event::Key(key) = event {
                if key.kind == crossterm::event::KeyEventKind::Press {
                    if self.handle_key(key).await == Handled::Quit {
                        break Ok(());
                    }
                }
            }
        }
    }

    async fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Handled {
        use crossterm::event::{KeyCode, KeyModifiers};

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Handled::Quit;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Handled::Quit,
            KeyCode::Tab => {
                self.panel = self.panel.next();
                Handled::Continue
            }
            KeyCode::Char('t') if self.panel == Panel::Details => {
                self.details_tab = match self.details_tab {
                    DetailsTab::Knowledge => DetailsTab::Entity,
                    DetailsTab::Entity => DetailsTab::Knowledge,
                };
                self.details_scroll = 0;
                Handled::Continue
            }
            KeyCode::Char('r') => {
                if let Err(error) = self.reload().await {
                    self.status = format!("refresh failed: {error}");
                }
                Handled::Continue
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                Handled::Continue
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                Handled::Continue
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.set_selection(0);
                Handled::Continue
            }
            KeyCode::Char('G') | KeyCode::End => {
                let last = self.visible_len().saturating_sub(1);
                self.set_selection(last);
                Handled::Continue
            }
            KeyCode::Char('d') | KeyCode::PageDown => {
                let delta = HALF_PAGE as isize;
                self.move_selection(delta);
                Handled::Continue
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                let delta = -(HALF_PAGE as isize);
                self.move_selection(delta);
                Handled::Continue
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_selected();
                Handled::Continue
            }
            _ => Handled::Continue,
        }
    }

    fn visible_len(&self) -> usize {
        match self.panel {
            Panel::Tree => self.visible.len(),
            Panel::Details => self.details_lines().len(),
            Panel::Diagnostics => self.diagnostics_lines().len(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        let current = match self.panel {
            Panel::Tree => self.selected,
            Panel::Details => self.details_scroll,
            Panel::Diagnostics => self.diagnostics_scroll,
        } as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.set_selection(next);
    }

    fn set_selection(&mut self, index: usize) {
        match self.panel {
            Panel::Tree => {
                self.selected = index.min(self.visible.len().saturating_sub(1));
                self.tree_state.select(Some(self.selected));
                self.details_scroll = 0;
            }
            Panel::Details => self.details_scroll = index,
            Panel::Diagnostics => self.diagnostics_scroll = index,
        }
    }

    fn toggle_selected(&mut self) {
        if self.panel != Panel::Tree {
            return;
        }
        let Some(id) = self.visible.get(self.selected).map(|(id, _)| *id) else {
            return;
        };
        if self.expanded.contains(&id) {
            self.expanded.remove(&id);
        } else {
            self.expanded.insert(id);
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.rebuild_visible(&root);
    }

    fn selected_index(&self) -> Option<&Index> {
        self.visible
            .get(self.selected)
            .and_then(|(id, _)| self.by_id.get(id))
    }

    fn selected_knowledge(&self) -> Option<&Knowledge> {
        self.selected_index()
            .and_then(|index| index.target)
            .and_then(|target| self.knowledge_by_id.get(&target))
    }

    fn selected_entities(&self) -> Vec<&Entity> {
        self.selected_knowledge()
            .map(|knowledge| {
                knowledge
                    .entities
                    .iter()
                    .filter_map(|id| self.entity_by_id.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn details_lines(&self) -> Vec<Line<'static>> {
        let index = self.selected_index();
        let mut lines = Vec::new();
        if let Some(index) = index {
            lines.push(Line::from(vec![
                Span::styled("Index: ", Style::default().fg(self.theme.muted)),
                Span::raw(
                    index
                        .title
                        .clone()
                        .unwrap_or_else(|| "(untitled)".to_string()),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Type: ", Style::default().fg(self.theme.muted)),
                Span::raw(match index.target_type {
                    TargetType::Group => "group",
                    TargetType::Knowledge => "knowledge",
                }),
            ]));
            lines.push(Line::from(vec![
                Span::styled("ID: ", Style::default().fg(self.theme.muted)),
                Span::raw(index.id.to_string()),
            ]));
            lines.push(Line::from(""));
        }

        match self.details_tab {
            DetailsTab::Knowledge => {
                if let Some(knowledge) = self.selected_knowledge() {
                    lines.push(Line::from(Span::styled(
                        knowledge.title.clone(),
                        Style::default()
                            .fg(self.theme.knowledge)
                            .add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(format!(
                        "type: {}",
                        knowledge.knowledge_type.as_str()
                    )));
                    lines.push(Line::from(""));
                    for line in knowledge.content.clone().unwrap_or_default().lines() {
                        lines.push(Line::from(line.to_string()));
                    }
                } else {
                    lines.push(Line::from(Span::styled(
                        "Select a knowledge leaf to inspect its content.",
                        Style::default().fg(self.theme.muted),
                    )));
                }
            }
            DetailsTab::Entity => {
                let entities = self.selected_entities();
                if entities.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No linked entities for this node.",
                        Style::default().fg(self.theme.muted),
                    )));
                }
                for entity in entities {
                    let names = entity
                        .name
                        .iter()
                        .map(|name| format!("{} [{}]", name.full, name.lang.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    lines.push(Line::from(Span::styled(
                        names,
                        Style::default()
                            .fg(self.theme.focus)
                            .add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(entity.definition.clone()));
                    lines.push(Line::from(format!("id: {}", entity.id)));
                    lines.push(Line::from(""));
                }
            }
        }
        lines
    }

    fn diagnostics_lines(&self) -> Vec<Line<'static>> {
        self.diagnostics
            .iter()
            .map(|diagnostic| {
                let color = match diagnostic.severity {
                    Severity::Error => self.theme.error,
                    Severity::Warning => self.theme.warning,
                    Severity::Information | Severity::Hint => self.theme.muted,
                };
                Line::from(vec![
                    Span::styled(
                        format!("{:<5}", diagnostic.severity.label()),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", diagnostic.code),
                        Style::default().fg(self.theme.muted),
                    ),
                    Span::raw(diagnostic.message.clone()),
                ])
            })
            .collect()
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(area);
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(columns[0]);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(columns[1]);

        self.render_tree(frame, left[0]);
        self.render_diagnostics(frame, left[1]);
        self.render_details(frame, right[0]);
        self.render_status(frame, right[1]);
    }

    fn render_tree(&mut self, frame: &mut Frame, area: Rect) {
        let items = self
            .visible
            .iter()
            .map(|(id, indent)| {
                let Some(index) = self.by_id.get(id) else {
                    return ListItem::new("(missing)");
                };
                let prefix = if index.target_type == TargetType::Knowledge {
                    "● "
                } else if self.expanded.contains(id) {
                    "▾ "
                } else {
                    "▸ "
                };
                ListItem::new(Line::from(vec![
                    Span::raw(" ".repeat(indent.saturating_sub(1) * 2)),
                    Span::styled(
                        prefix,
                        Style::default().fg(if index.target_type == TargetType::Knowledge {
                            self.theme.knowledge
                        } else {
                            self.theme.group
                        }),
                    ),
                    Span::raw(
                        index
                            .title
                            .clone()
                            .unwrap_or_else(|| "(untitled)".to_string()),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Tree ")
                    .borders(Borders::ALL)
                    .border_style(self.border_style(Panel::Tree)),
            )
            .highlight_style(
                Style::default()
                    .bg(self.theme.selected)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .scroll_padding(2);
        frame.render_stateful_widget(list, area, &mut self.tree_state);
    }

    fn render_details(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        let active = Style::default()
            .fg(self.theme.focus)
            .add_modifier(Modifier::BOLD);
        let inactive = Style::default().fg(self.theme.muted);
        let (knowledge_style, entity_style) = match self.details_tab {
            DetailsTab::Knowledge => (active, inactive),
            DetailsTab::Entity => (inactive, active),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Knowledge ", knowledge_style),
                Span::styled("|", inactive),
                Span::styled(" Entity ", entity_style),
                Span::styled(" [t]", inactive),
            ])),
            chunks[0],
        );
        let lines = self.details_lines();
        let visible_height = chunks[1].height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(visible_height);
        self.details_scroll = self.details_scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Knowledge / Entity ")
                        .borders(Borders::ALL)
                        .border_style(self.border_style(Panel::Details)),
                )
                .scroll((self.details_scroll as u16, 0))
                .wrap(Wrap { trim: false }),
            chunks[1],
        );
    }

    fn render_diagnostics(&mut self, frame: &mut Frame, area: Rect) {
        let lines = self.diagnostics_lines();
        let visible_height = area.height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(visible_height);
        self.diagnostics_scroll = self.diagnostics_scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Diagnostics ")
                        .borders(Borders::ALL)
                        .border_style(self.border_style(Panel::Diagnostics)),
                )
                .scroll((self.diagnostics_scroll as u16, 0))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let help = "Tab panel | j/k move | Space expand | t detail tab | r refresh | q quit";
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", self.status),
                    Style::default().fg(self.theme.focus),
                ),
                Span::styled(help, Style::default().fg(self.theme.muted)),
            ])),
            area,
        );
    }

    fn border_style(&self, panel: Panel) -> Style {
        if self.panel == panel {
            Style::default().fg(self.theme.focus)
        } else {
            Style::default().fg(self.theme.muted)
        }
    }
}

#[derive(PartialEq)]
enum Handled {
    Continue,
    Quit,
}

fn index_tree(node: &TreeNode, map: &mut HashMap<Uuid, Index>) {
    map.insert(node.value.id, node.value.clone());
    for child in &node.children {
        index_tree(child, map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(id: Uuid, title: &str, target_type: TargetType) -> Index {
        Index {
            id,
            title: Some(title.to_string()),
            target: None,
            target_type,
            parent_id: None,
            position: 0,
        }
    }

    #[test]
    fn flatten_respects_expanded_nodes() {
        let child_id = Uuid::new_v4();
        let grandchild_id = Uuid::new_v4();
        let grandchild = TreeNode {
            value: index(grandchild_id, "Grandchild", TargetType::Knowledge),
            children: Vec::new(),
        };
        let child = TreeNode {
            value: index(child_id, "Child", TargetType::Group),
            children: vec![grandchild],
        };
        let root = TreeNode {
            value: index(Uuid::new_v4(), "Root", TargetType::Group),
            children: vec![child],
        };

        let mut expanded = HashSet::new();
        expanded.insert(root.value.id);
        let mut flat = Vec::new();
        root.flatten(&expanded, 0, &mut flat);
        assert_eq!(flat.len(), 2);

        expanded.insert(child_id);
        let mut flat = Vec::new();
        root.flatten(&expanded, 0, &mut flat);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[2].1, 2);
    }

    #[tokio::test]
    async fn initializes_kms_tables_in_agent_database() {
        let dir = std::env::temp_dir().join(format!(
            "autonomics-kms-tui-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        let app = KmsTui::new(&db).await.unwrap();

        assert_eq!(app.visible.len(), 1);
        assert!(app.status.contains("1 node"));
        assert!(app.status.contains("0 knowledge"));
        assert!(app.status.contains("0 diagnostics"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
