use bib_base::Article;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{layout::Rect, prelude::Buffer, widgets::StatefulWidget as _};

use super::*;

#[test]
fn filters_by_title_journal_keyword_and_doi() {
    let mut genetics =
        Article::new("genetics", "Genetic associations").with_doi("10.1000/genetics");
    genetics.journal = Some("Nature Genetics".to_owned());
    let mut methods = Article::new("methods", "Study design");
    methods.keywords = vec!["survey".to_owned()];
    let mut state = BibliographyState::new(vec![genetics, methods]);

    state.set_query("nature");
    assert_eq!(state.matched_articles().count(), 1);
    state.set_query("survey");
    assert_eq!(state.matched_articles().count(), 1);
    state.set_query("10.1000/genetics");
    assert_eq!(state.matched_articles().count(), 1);
    state.set_query("");
    assert_eq!(state.matched_articles().count(), 2);
}

#[test]
fn enter_requests_open_action() {
    let mut state = BibliographyState::new(vec![Article::new("article-1", "Selected")]);
    state.set_focus(BibliographyFocus::Results);
    state.handle_key(KeyEvent::from(KeyCode::Enter));

    assert_eq!(
        state.take_action(),
        Some(BibliographyAction::OpenArticle {
            article_id: "article-1".to_owned()
        })
    );
    assert_eq!(state.take_action(), None);
}

#[test]
fn renders_list_and_details_without_panicking() {
    let mut article = Article::new("article-1", "Terminal bibliography management");
    article.keywords = vec!["tui".to_owned()];
    let mut state = BibliographyState::new(vec![article]);
    state.set_focus(BibliographyFocus::Results);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    BibliographyWidget::new().render(Rect::new(0, 0, 80, 24), &mut buffer, &mut state);

    let content: String = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();
    assert!(content.contains("Bibliography"));
}
