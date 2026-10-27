//! Bibliography management TUI.
//!
//! [`BibliographyWidget`] is the only public widget. The command loop owns its
//! state, while private child widgets render the search bar, collections,
//! article list, details tabs, and footer. This keeps the feature composable
//! internally without leaking five separate widgets into the application.

mod article_list;
mod article_pane;
mod collection_pane;
mod search_bar;
mod status_bar;

use std::collections::{HashMap, HashSet};

use bib_base::{Annotation, Article, Collection, FullText};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Buffer, StatefulWidget, Widget},
    style::Color,
};

use article_list::ArticleList;
use article_pane::ArticlePane;
use collection_pane::CollectionPane;
use search_bar::SearchBar;
use status_bar::StatusBar;

/// Pane currently receiving navigation/editing keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BibliographyFocus {
    #[default]
    Articles,
    Collections,
    Search,
    Details,
}

impl BibliographyFocus {
    fn next(self) -> Self {
        match self {
            Self::Articles => Self::Details,
            Self::Details => Self::Collections,
            Self::Collections => Self::Search,
            Self::Search => Self::Articles,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Articles => Self::Search,
            Self::Search => Self::Collections,
            Self::Collections => Self::Details,
            Self::Details => Self::Articles,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Articles => "articles",
            Self::Collections => "collections",
            Self::Search => "search",
            Self::Details => "details",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailsTab {
    #[default]
    Metadata,
    FullText,
    Notes,
}

impl DetailsTab {
    /// Render order of the details tab bar.
    pub(super) const TABS: [Self; 3] = [Self::Metadata, Self::FullText, Self::Notes];

    fn next(self) -> Self {
        match self {
            Self::Metadata => Self::FullText,
            Self::FullText => Self::Notes,
            Self::Notes => Self::Metadata,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Metadata => Self::Notes,
            Self::Notes => Self::FullText,
            Self::FullText => Self::Metadata,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Metadata => "Metadata",
            Self::FullText => "Full text",
            Self::Notes => "Notes",
        }
    }
}

/// A renderable bibliography snapshot. Database loading happens outside the
/// widget so rendering is synchronous and easily testable.
#[derive(Debug, Clone, Default)]
pub struct BibliographyState {
    pub collections: Vec<Collection>,
    pub expanded_collections: HashSet<String>,
    pub include_subcollections: bool,
    pub articles: Vec<Article>,
    /// `None` means the virtual “All articles” collection.
    pub selected_collection: Option<String>,
    pub selected_article: Option<String>,
    pub query: String,
    pub authoritative_query: String,
    pub focus: BibliographyFocus,
    pub details_tab: DetailsTab,
    pub details_scroll: u16,
    pub fulltexts: HashMap<String, FullText>,
    pub annotations: HashMap<String, Vec<Annotation>>,
    pub status: String,
    /// Set by pressing Enter in search; consumed by the command loop.
    pub pending_search_reload: bool,
    pub error: Option<String>,
}

impl BibliographyState {
    pub fn set_snapshot(&mut self, collections: Vec<Collection>, articles: Vec<Article>) {
        self.collections = collections;
        self.articles = articles;
        if let Some(id) = &self.selected_collection {
            if !self.collections.iter().any(|c| &c.id == id) {
                self.selected_collection = None;
            }
        }
        let mut ancestor = self.selected_collection.clone();
        let mut seen = HashSet::new();
        while let Some(id) = ancestor {
            if !seen.insert(id.clone()) {
                break;
            }
            ancestor = self
                .collections
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.parent_id.clone());
            if let Some(parent) = &ancestor {
                self.expanded_collections.insert(parent.clone());
            }
        }
        if let Some(id) = &self.selected_article {
            if !self.articles.iter().any(|a| &a.id == id) {
                self.selected_article = None;
            }
        }
        if self.selected_article.is_none() {
            self.selected_article = self.first_filtered_article();
        }
        self.details_scroll = 0;
    }

    /// Client-side refinement over a bounded library snapshot. The initial
    /// database query remains the authoritative search path.
    pub fn filtered_articles(&self) -> impl Iterator<Item = &Article> {
        let collection = self.selected_collection.as_deref();
        let query_words = query_words(&self.query);
        let mut scope: HashSet<&str> = collection.into_iter().collect();
        if self.include_subcollections {
            let mut changed = true;
            while changed {
                changed = false;
                for c in &self.collections {
                    if !c.deleted
                        && c.parent_id.as_deref().is_some_and(|p| scope.contains(p))
                        && scope.insert(c.id.as_str())
                    {
                        changed = true;
                    }
                }
            }
        }
        self.articles.iter().filter(move |article| {
            let in_collection = collection.is_none_or(|_| {
                self.collections
                    .iter()
                    .filter(|c| scope.contains(c.id.as_str()) && !c.deleted)
                    .flat_map(|c| c.article_ids.iter())
                    .any(|member| member == &article.id)
            });
            in_collection
                && (self.query == self.authoritative_query
                    || query_words.iter().all(|word| {
                        article.title.to_lowercase().contains(word)
                            || article
                                .abstract_text
                                .as_deref()
                                .is_some_and(|text| text.to_lowercase().contains(word))
                            || article
                                .keywords
                                .iter()
                                .any(|k| k.to_lowercase().contains(word))
                    }))
        })
    }

    fn first_filtered_article(&self) -> Option<String> {
        self.filtered_articles()
            .next()
            .map(|article| article.id.clone())
    }

    pub fn selected_article(&self) -> Option<&Article> {
        let id = self.selected_article.as_deref()?;
        self.articles.iter().find(|article| article.id == id)
    }

    pub fn select_article(&mut self, id: impl Into<String>) {
        self.selected_article = Some(id.into());
        self.details_scroll = 0;
    }

    pub fn move_article(&mut self, forward: bool) {
        let ids: Vec<String> = self.filtered_articles().map(|a| a.id.clone()).collect();
        if ids.is_empty() {
            self.selected_article = None;
            return;
        }
        let next = match self
            .selected_article
            .as_deref()
            .and_then(|id| ids.iter().position(|x| x == id))
        {
            Some(index) if forward => (index + 1) % ids.len(),
            Some(0) if !forward => ids.len() - 1,
            Some(index) => index - 1,
            None => 0,
        };
        self.select_article(ids[next].clone());
    }

    /// One shared visible order for rendering and keyboard navigation.
    pub fn collection_rows(&self) -> Vec<(&Collection, usize)> {
        let mut children: HashMap<Option<&str>, Vec<&Collection>> = HashMap::new();
        let live: HashSet<&str> = self
            .collections
            .iter()
            .filter(|c| !c.deleted)
            .map(|c| c.id.as_str())
            .collect();
        for c in self.collections.iter().filter(|c| !c.deleted) {
            let parent = c.parent_id.as_deref().filter(|id| live.contains(id));
            children.entry(parent).or_default().push(c);
        }
        for group in children.values_mut() {
            group.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.id.cmp(&b.id))
            });
        }
        let mut stack: Vec<(&Collection, usize)> = children
            .remove(&None)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(|c| (c, 0))
            .collect();
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        while let Some((c, depth)) = stack.pop() {
            if !seen.insert(c.id.as_str()) {
                continue;
            }
            rows.push((c, depth));
            if self.expanded_collections.contains(&c.id) {
                for child in children
                    .remove(&Some(c.id.as_str()))
                    .unwrap_or_default()
                    .into_iter()
                    .rev()
                {
                    stack.push((child, depth + 1));
                }
            }
        }
        rows
    }

    pub fn move_collection(&mut self, forward: bool) {
        let ids: Vec<String> = self
            .collection_rows()
            .iter()
            .map(|(c, _)| c.id.clone())
            .collect();
        let current = self
            .selected_collection
            .as_ref()
            .and_then(|id| ids.iter().position(|c| c == id))
            .map(|i| i + 1)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % (ids.len() + 1)
        } else if current == 0 {
            ids.len()
        } else {
            current - 1
        };
        self.selected_collection = if next == 0 {
            None
        } else {
            Some(ids[next - 1].clone())
        };
        self.selected_article = self.first_filtered_article();
        self.details_scroll = 0;
    }

    pub fn set_details(
        &mut self,
        article_id: String,
        fulltext: Option<FullText>,
        annotations: Vec<Annotation>,
    ) {
        if let Some(fulltext) = fulltext {
            self.fulltexts.insert(article_id.clone(), fulltext);
        }
        self.annotations.insert(article_id, annotations);
    }

    /// Returns `true` when the selected article changed and its details need loading.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return false;
        }
        match key.code {
            KeyCode::Tab => {
                self.focus = self.focus.next();
                false
            }
            KeyCode::BackTab => {
                self.focus = self.focus.previous();
                false
            }
            KeyCode::Char('/') if self.focus != BibliographyFocus::Search => {
                self.focus = BibliographyFocus::Search;
                false
            }
            KeyCode::Esc if self.focus == BibliographyFocus::Search => {
                self.focus = BibliographyFocus::Articles;
                false
            }
            KeyCode::Enter if self.focus == BibliographyFocus::Search => {
                self.focus = BibliographyFocus::Articles;
                self.pending_search_reload = true;
                false
            }
            KeyCode::Backspace if self.focus == BibliographyFocus::Search => {
                self.query.pop();
                self.refine_selection()
            }
            KeyCode::Char(ch) if self.focus == BibliographyFocus::Search => {
                if !ch.is_control() {
                    self.query.push(ch);
                    self.refine_selection()
                } else {
                    false
                }
            }
            KeyCode::Left | KeyCode::Char('h') => match self.focus {
                BibliographyFocus::Details => {
                    self.details_tab = self.details_tab.previous();
                    self.details_scroll = 0;
                    false
                }
                BibliographyFocus::Articles | BibliographyFocus::Search => {
                    self.focus = self.focus.previous();
                    false
                }
                BibliographyFocus::Collections => {
                    if let Some(id) = self.selected_collection.clone() {
                        if !self.expanded_collections.remove(&id) {
                            self.selected_collection = self
                                .collections
                                .iter()
                                .find(|c| c.id == id)
                                .and_then(|c| c.parent_id.clone());
                            self.selected_article = self.first_filtered_article();
                        }
                    }
                    false
                }
            },
            KeyCode::Right | KeyCode::Char('l') => match self.focus {
                BibliographyFocus::Details => {
                    self.details_tab = self.details_tab.next();
                    self.details_scroll = 0;
                    false
                }
                BibliographyFocus::Articles | BibliographyFocus::Search => {
                    self.focus = self.focus.next();
                    false
                }
                BibliographyFocus::Collections => {
                    if let Some(id) = self.selected_collection.clone() {
                        if self
                            .collections
                            .iter()
                            .any(|c| c.parent_id.as_deref() == Some(id.as_str()))
                        {
                            if !self.expanded_collections.insert(id) {
                                self.move_collection(true);
                            }
                        }
                    }
                    false
                }
            },
            KeyCode::Char('R') if self.focus == BibliographyFocus::Collections => {
                self.include_subcollections = !self.include_subcollections;
                self.selected_article = self.first_filtered_article();
                false
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_forward(),
            KeyCode::Up | KeyCode::Char('k') => self.move_backward(),
            KeyCode::PageDown => {
                for _ in 0..10 {
                    self.move_forward();
                }
                false
            }
            KeyCode::PageUp => {
                for _ in 0..10 {
                    self.move_backward();
                }
                false
            }
            _ => false,
        }
    }

    fn move_forward(&mut self) -> bool {
        match self.focus {
            BibliographyFocus::Articles | BibliographyFocus::Search => {
                let before = self.selected_article.clone();
                self.move_article(true);
                before != self.selected_article
            }
            BibliographyFocus::Collections => {
                self.move_collection(true);
                false
            }
            BibliographyFocus::Details => {
                self.details_scroll = self.details_scroll.saturating_add(1);
                false
            }
        }
    }

    fn move_backward(&mut self) -> bool {
        match self.focus {
            BibliographyFocus::Articles | BibliographyFocus::Search => {
                let before = self.selected_article.clone();
                self.move_article(false);
                before != self.selected_article
            }
            BibliographyFocus::Collections => {
                self.move_collection(false);
                false
            }
            BibliographyFocus::Details => {
                self.details_scroll = self.details_scroll.saturating_sub(1);
                false
            }
        }
    }

    fn refine_selection(&mut self) -> bool {
        let valid = self
            .selected_article
            .as_deref()
            .is_some_and(|id| self.filtered_articles().any(|a| a.id == id));
        if !valid {
            let before = self.selected_article.clone();
            self.selected_article = self.first_filtered_article();
            self.details_scroll = 0;
            before != self.selected_article
        } else {
            false
        }
    }
}

fn query_words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|word| word.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}

/// Public root widget. All child widgets are private implementation details.
#[derive(Debug, Clone)]
pub struct BibliographyWidget {
    pub accent: Color,
}

impl Default for BibliographyWidget {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
        }
    }
}

impl StatefulWidget for BibliographyWidget {
    type State = BibliographyState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 48 || area.height < 12 {
            let compact = ratatui::widgets::Paragraph::new(format!(
                "bibliography needs 48x12 (have {}x{})",
                area.width, area.height
            ))
            .style(ratatui::style::Style::new().fg(Color::Yellow));
            ratatui::prelude::Widget::render(compact, area, buf);
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(24),
                Constraint::Percentage(42),
                // Wide enough for the full tab bar (30 cols) to stay readable.
                Constraint::Min(30),
            ])
            .split(rows[1]);

        SearchBar {
            query: state.query.as_str(),
            focused: state.focus == BibliographyFocus::Search,
            result_count: state.filtered_articles().count(),
            accent: self.accent,
        }
        .render(rows[0], buf);

        let collection_rows = state.collection_rows();
        CollectionPane {
            rows: &collection_rows,
            expanded: &state.expanded_collections,
            recursive: state.include_subcollections,
            selected: state.selected_collection.as_deref(),
            focused: state.focus == BibliographyFocus::Collections,
            accent: self.accent,
        }
        .render(columns[0], buf);

        let mut article_list_state = state.list_state();
        ArticleList {
            articles: state.articles.as_slice(),
            visible: state.filtered_articles().map(|a| a.id.as_str()).collect(),
            selected: state.selected_article.as_deref(),
            fulltexts: &state.fulltexts,
            focused: state.focus == BibliographyFocus::Articles,
            accent: self.accent,
        }
        .render(columns[1], buf, &mut article_list_state);

        ArticlePane {
            article: state.selected_article(),
            fulltext: state
                .selected_article()
                .and_then(|a| state.fulltexts.get(&a.id)),
            annotations: state
                .selected_article()
                .and_then(|a| state.annotations.get(&a.id).map(Vec::as_slice)),
            tab: state.details_tab,
            focused: state.focus == BibliographyFocus::Details,
            scroll: state.details_scroll,
            accent: self.accent,
        }
        .render(columns[2], buf);

        StatusBar {
            focus: state.focus.label(),
            selected_id: state.selected_article.as_deref(),
            status: state.status.as_str(),
            error: state.error.as_deref(),
            accent: self.accent,
        }
        .render(rows[2], buf);
    }
}

impl BibliographyState {
    fn list_state(&self) -> ratatui::widgets::ListState {
        let mut list_state = ratatui::widgets::ListState::default();
        if let Some(id) = &self.selected_article {
            let position = self
                .filtered_articles()
                .position(|article| &article.id == id);
            list_state.select(position);
        }
        list_state
    }
}

#[cfg(test)]
mod tests;
