//! Keyboard handling for the bibliography workspace.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{BibliographyAction, BibliographyFocus, BibliographyState};

const DETAILS_PAGE_SIZE: u16 = 10;

impl BibliographyState {
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.handle_control_key(key.code);
            return;
        }

        match key.code {
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::Char('/') if self.focus != BibliographyFocus::Search => {
                self.focus = BibliographyFocus::Search;
            }
            KeyCode::Esc => self.handle_escape(),
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                BibliographyFocus::Results => self.move_up(),
                BibliographyFocus::Details => self.scroll_details_up(),
                BibliographyFocus::Search => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                BibliographyFocus::Results => self.move_down(),
                BibliographyFocus::Details => self.scroll_details_down(),
                BibliographyFocus::Search => {}
            },
            KeyCode::Home if self.focus == BibliographyFocus::Results => self.move_to_start(),
            KeyCode::End if self.focus == BibliographyFocus::Results => self.move_to_end(),
            KeyCode::PageUp if self.focus == BibliographyFocus::Details => {
                self.details_scroll = self.details_scroll.saturating_sub(DETAILS_PAGE_SIZE);
            }
            KeyCode::PageDown if self.focus == BibliographyFocus::Details => {
                self.details_scroll = self.details_scroll.saturating_add(DETAILS_PAGE_SIZE);
            }
            KeyCode::Backspace if self.focus == BibliographyFocus::Search => {
                self.query.pop();
                self.rebuild_matches();
            }
            KeyCode::Enter if self.focus == BibliographyFocus::Search => {
                self.focus = BibliographyFocus::Results;
            }
            KeyCode::Enter if self.focus == BibliographyFocus::Results => {
                if let Some(article_id) = self.selected_article_id() {
                    self.pending_action = Some(BibliographyAction::OpenArticle {
                        article_id: article_id.to_owned(),
                    });
                } else {
                    self.status_message = Some("No article selected".to_owned());
                }
            }
            KeyCode::Enter if self.focus == BibliographyFocus::Details => {
                self.focus = BibliographyFocus::Results;
            }
            KeyCode::Char(character) if self.focus == BibliographyFocus::Search => {
                self.query.push(character);
                self.rebuild_matches();
            }
            _ => {}
        }
    }

    fn handle_control_key(&mut self, key_code: KeyCode) {
        if matches!(key_code, KeyCode::Char('h') | KeyCode::Char('l')) {
            self.status_message = Some("Horizontal detail scrolling is not implemented".to_owned());
        }
    }

    fn handle_escape(&mut self) {
        match self.focus {
            BibliographyFocus::Search if !self.query.is_empty() => {
                self.query.clear();
                self.rebuild_matches();
            }
            BibliographyFocus::Search => self.focus = BibliographyFocus::Results,
            BibliographyFocus::Results => self.focus = BibliographyFocus::Search,
            BibliographyFocus::Details => self.focus = BibliographyFocus::Results,
        }
    }
}
