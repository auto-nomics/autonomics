//! Bibliography popup: daemon-backed browsing and asynchronous data loading.

use ratatui::prelude::StatefulWidget;

use super::*;

impl App {
    pub(super) fn open_bibliography(&mut self) {
        self.state.bibliography_visible = true;
        self.refresh_bibliography_snapshot();
    }

    pub(super) fn close_bibliography(&mut self) {
        self.state.bibliography_visible = false;
    }

    /// Fetch collections and up to 500 articles matching the current search.
    /// The request token makes an older reply harmless when refreshes race.
    fn refresh_bibliography_snapshot(&mut self) {
        let query = self.state.bibliography_state.query.trim().to_owned();
        self.state.bibliography_snapshot_token =
            self.state.bibliography_snapshot_token.wrapping_add(1);
        let token = self.state.bibliography_snapshot_token;
        self.state.bibliography_state.status = "Loading bibliography…".to_owned();
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("bibliography_snapshot", move || async move {
            let result = client.bib_snapshot(&query).await;
            event_tx.send(crate::app_event::AppEvent::BibliographySnapshotLoaded {
                token,
                result: result.map_err(|error| error.to_string()),
            });
        });
    }

    /// Fetch full-text/annotation details for a newly selected article. Each
    /// request carries a monotonic token so a stale response cannot overwrite
    /// details for the article currently selected in the popup.
    fn fetch_bibliography_details(&mut self) {
        if !self.state.bibliography_visible {
            return;
        }
        let Some(article_id) = self.state.bibliography_state.selected_article.clone() else {
            return;
        };

        self.state.bibliography_detail_token = self.state.bibliography_detail_token.wrapping_add(1);
        let token = self.state.bibliography_detail_token;
        let client = self.client.clone();
        let event_tx = self.app_event_tx.clone();
        self.spawn_client_task("bibliography_details", move || async move {
            let result = client.bib_article_detail(&article_id).await;
            event_tx.send(crate::app_event::AppEvent::BibliographyDetailsLoaded {
                token,
                article_id,
                result: result.map_err(|error| error.to_string()),
            });
        });
    }

    pub(super) fn handle_bibliography_key(&mut self, key: &KeyEvent) {
        if key.code == KeyCode::Esc {
            self.close_bibliography();
            return;
        }
        if key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('q')) {
            self.close_bibliography();
            return;
        }
        if key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('r' | 'R')) {
            self.refresh_bibliography_snapshot();
            return;
        }

        let before = self.state.bibliography_state.selected_article.clone();
        let query_before = self.state.bibliography_state.query.clone();
        let detail_needed = self.state.bibliography_state.handle_key(*key);
        let selected_after = self.state.bibliography_state.selected_article.clone();
        let search_reload = self.state.bibliography_state.pending_search_reload;
        let query_after = self.state.bibliography_state.query.clone();

        if detail_needed || before != selected_after {
            self.fetch_bibliography_details();
            return;
        }
        if search_reload {
            self.state.bibliography_state.pending_search_reload = false;
            self.refresh_bibliography_snapshot();
        } else if query_before != query_after {
            // Typing filters the existing bounded snapshot locally. Enter
            // performs the authoritative daemon-side search.
            self.state.bibliography_state.status = format!(
                "{} articles loaded",
                self.state.bibliography_state.articles.len()
            );
        }
    }

    pub(super) fn render_bibliography(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::prelude::Buffer,
    ) {
        let popup_width = area.width.saturating_sub(4).min(area.width);
        let popup_height = area.height.saturating_sub(3).min(area.height);
        let popup = crate::widgets::popup::Popup::new(
            " Bibliography ",
            crate::widgets::popup::PopupControls::default(),
        )
        .width(popup_width)
        .height(popup_height)
        .vertically_centered(true)
        .accent(ratatui::style::Color::Cyan);
        let inner = popup.render(area, buf);
        crate::widgets::bibliography::BibliographyWidget::default().render(
            inner,
            buf,
            &mut self.state.bibliography_state,
        );
    }

    /// Apply a snapshot response if it is still current. Selecting the first
    /// result after a load also requests its details.
    pub(super) fn apply_bibliography_snapshot(
        &mut self,
        token: u64,
        result: Result<gateway::proto::BibliographySnapshot, String>,
    ) {
        if token != self.state.bibliography_snapshot_token {
            return;
        }
        let bibliography = &mut self.state.bibliography_state;
        match result {
            Ok(snapshot) => {
                let selected_before = bibliography.selected_article.clone();
                bibliography.set_snapshot(snapshot.collections, snapshot.articles);
                bibliography.status = format!("{} articles loaded", bibliography.articles.len());
                bibliography.error = None;
                if selected_before != bibliography.selected_article {
                    self.fetch_bibliography_details();
                } else if let Some(article_id) = bibliography.selected_article.clone() {
                    self.fetch_article_details_if_missing(article_id);
                }
            }
            Err(error) => {
                bibliography.status.clear();
                bibliography.error = Some(error);
            }
        }
    }

    fn fetch_article_details_if_missing(&mut self, article_id: String) {
        if !self
            .state
            .bibliography_state
            .fulltexts
            .contains_key(&article_id)
        {
            self.fetch_bibliography_details();
        }
    }

    pub(super) fn apply_bibliography_details(
        &mut self,
        token: u64,
        article_id: String,
        result: Result<gateway::proto::BibliographyArticleDetail, String>,
    ) {
        if token != self.state.bibliography_detail_token
            || self.state.bibliography_state.selected_article.as_deref()
                != Some(article_id.as_str())
        {
            return;
        }
        let bibliography = &mut self.state.bibliography_state;
        match result {
            Ok(detail) => {
                bibliography.set_details(detail.article.id, detail.fulltext, detail.annotations);
                bibliography.error = None;
            }
            Err(error) => bibliography.error = Some(error),
        }
    }
}
