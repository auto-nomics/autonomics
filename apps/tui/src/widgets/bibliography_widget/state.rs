//! State owned by the bibliography workspace.

use bib_base::Article;
use ratatui::widgets::ListState;

use super::query::article_matches;

/// Panes owned by the bibliography widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BibliographyFocus {
    #[default]
    Search,
    Results,
    Details,
}

impl BibliographyFocus {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Search => Self::Results,
            Self::Results => Self::Details,
            Self::Details => Self::Search,
        }
    }

    pub(super) fn previous(self) -> Self {
        match self {
            Self::Search => Self::Details,
            Self::Results => Self::Search,
            Self::Details => Self::Results,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Results => "Results",
            Self::Details => "Details",
        }
    }
}

/// Command requested by widget key handling. The caller owns execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibliographyAction {
    OpenArticle { article_id: String },
}

#[derive(Default)]
pub struct BibliographyState {
    pub(super) articles: Vec<Article>,
    pub(super) matches: Vec<usize>,
    pub(super) query: String,
    pub(super) selected: usize,
    pub(super) list_state: ListState,
    pub(super) focus: BibliographyFocus,
    pub(super) details_scroll: u16,
    pub(super) pending_action: Option<BibliographyAction>,
    pub(super) status_message: Option<String>,
}

impl BibliographyState {
    pub fn new(articles: Vec<Article>) -> Self {
        let mut state = Self {
            articles,
            ..Self::default()
        };
        state.rebuild_matches();
        state
    }

    pub fn set_articles(&mut self, articles: Vec<Article>) {
        self.articles = articles;
        self.rebuild_matches();
        self.status_message = None;
    }

    pub fn articles(&self) -> &[Article] {
        &self.articles
    }

    pub fn article_count(&self) -> usize {
        self.articles.len()
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn matched_articles(&self) -> impl Iterator<Item = &Article> {
        self.matches
            .iter()
            .filter_map(|index| self.articles.get(*index))
    }

    pub fn selected_article(&self) -> Option<&Article> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.articles.get(*index))
    }

    pub fn selected_article_id(&self) -> Option<&str> {
        self.selected_article().map(|article| article.id.as_str())
    }

    pub fn focus(&self) -> BibliographyFocus {
        self.focus
    }

    pub fn set_focus(&mut self, focus: BibliographyFocus) {
        self.focus = focus;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.rebuild_matches();
    }

    pub fn details_scroll(&self) -> u16 {
        self.details_scroll
    }

    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.matches.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    pub fn move_to_start(&mut self) {
        self.selected = 0;
        self.sync_list_state();
    }

    pub fn move_to_end(&mut self) {
        self.selected = self.matches.len().saturating_sub(1);
        self.sync_list_state();
    }

    pub fn scroll_details_up(&mut self) {
        self.details_scroll = self.details_scroll.saturating_sub(1);
    }

    pub fn scroll_details_down(&mut self) {
        self.details_scroll = self.details_scroll.saturating_add(1);
    }

    pub fn take_action(&mut self) -> Option<BibliographyAction> {
        self.pending_action.take()
    }

    pub(super) fn list_state_mut(&mut self) -> &mut ListState {
        &mut self.list_state
    }

    pub(super) fn rebuild_matches(&mut self) {
        let needle = self.query.trim().to_lowercase();
        self.matches = self
            .articles
            .iter()
            .enumerate()
            .filter(|(_, article)| needle.is_empty() || article_matches(article, &needle))
            .map(|(index, _)| index)
            .collect();

        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        self.details_scroll = 0;
        self.sync_list_state();
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.matches.is_empty() || self.selected >= self.matches.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }
}
