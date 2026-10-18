use super::*;
use bib_base::{Annotation, AnnotationKind, Article, Author, Collection, FullText};

fn article(id: &str, title: &str, keywords: &[&str]) -> Article {
    let mut article = Article::new(id, title);
    article.year = Some(2025);
    article.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("J".into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    article.keywords = keywords.iter().map(|k| (*k).to_string()).collect();
    article.abstract_text = Some("population genetics abstract".into());
    article
}

fn snapshot() -> BibliographyState {
    let mut gwas = Collection::new("col-gwas", "GWAS reading list");
    gwas.article_ids = vec!["a1".into(), "a2".into()];
    let mut methods = Collection::new("col-methods", "Methods");
    methods.article_ids = vec!["a3".into()];
    let mut state = BibliographyState {
        status: "3 articles".into(),
        ..Default::default()
    };
    state.set_snapshot(
        vec![gwas, methods],
        vec![
            article("a1", "Polygenic risk models", &["prs"]),
            article("a2", "UK Biobank GWAS", &["gwas"]),
            article("a3", "Causal inference methods", &["methods"]),
        ],
    );
    state
}

fn render(state: &mut BibliographyState, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend, widgets::StatefulWidget as _};
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| BibliographyWidget::default().render(frame.area(), frame.buffer_mut(), state))
        .unwrap();
    let mut text = String::new();
    for y in 0..height {
        for x in 0..width {
            let cell = &terminal.backend().buffer()[(x, y)];
            if cell.symbol().is_empty() {
                text.push(' ');
            } else {
                text.push_str(cell.symbol());
            }
        }
        text.push('\n');
    }
    text
}

#[test]
fn filters_and_keeps_selection_valid() {
    let mut state = snapshot();
    assert_eq!(state.selected_article.as_deref(), Some("a1"));
    state.query = "UK gwas".into();
    state.refine_selection();
    assert_eq!(state.selected_article.as_deref(), Some("a2"));
    state.query = "nothing".into();
    state.refine_selection();
    assert_eq!(state.selected_article, None);
}

#[test]
fn collection_navigation_updates_article_selection() {
    let mut state = snapshot();
    state.focus = BibliographyFocus::Collections;
    state.move_collection(true);
    assert_eq!(state.selected_collection.as_deref(), Some("col-gwas"));
    assert_eq!(state.selected_article.as_deref(), Some("a1"));
    state.move_collection(true);
    assert_eq!(state.selected_collection.as_deref(), Some("col-methods"));
    assert_eq!(state.selected_article.as_deref(), Some("a3"));
    state.move_collection(true);
    assert_eq!(state.selected_collection, None);
}

#[test]
fn search_refinement_requests_detail_reload() {
    let mut state = snapshot();
    state.focus = BibliographyFocus::Search;
    // A single "u" is too short to filter on, so nothing moves yet.
    assert!(!state.handle_key(KeyEvent::from(KeyCode::Char('u'))));
    // Completing "uk" narrows to one article: selection moves, details reload.
    assert!(state.handle_key(KeyEvent::from(KeyCode::Char('k'))));
    assert_eq!(state.selected_article.as_deref(), Some("a2"));
    // A word matching nothing clears the selection (and requests a reload).
    assert!(state.handle_key(KeyEvent::from(KeyCode::Char('z'))));
    assert_eq!(state.selected_article, None);
    // Backspace restores the "uk" match set and reselects its first row.
    assert!(state.handle_key(KeyCode::Backspace.into()));
    assert_eq!(state.selected_article.as_deref(), Some("a2"));
    // Extra spaces add no constraint: no movement, no reload.
    assert!(!state.handle_key(KeyEvent::from(KeyCode::Char(' '))));
}

#[test]
fn renders_all_child_panes_without_overlap() {
    let mut state = snapshot();
    let mut ft = FullText {
        article_id: "a1".into(),
        file_path: "/articles/a1.pdf".into(),
        file_format: bib_base::FileFormat::Pdf,
        text_content: Some("Extracted introduction.".into()),
        source: bib_base::FullTextSource::UserUpload,
        file_hash: None,
        file_size: Some(123),
        uploaded_at: None,
        extract_status: Some(bib_base::ExtractStatus::Done),
        text_format: Some(bib_base::TextFormat::Plain),
        extracted_by: Some("pdf".into()),
        extract_error: None,
    };
    ft.extract_status = Some(bib_base::ExtractStatus::Done);
    state.fulltexts.insert("a1".into(), ft);
    state.annotations.insert(
        "a1".into(),
        vec![Annotation {
            id: "n1".into(),
            kind: AnnotationKind::Note,
            content: "Check methods".into(),
            page: Some(2),
            created_at: None,
        }],
    );
    state.details_tab = DetailsTab::Notes;
    let text = render(&mut state, 120, 28);
    for expected in [
        "Search",
        "Collections",
        "All articles",
        "Articles (3)",
        "Polygenic risk models",
        "Smith J",
        // The tab bar keeps every tab visible even when Notes is active.
        "Metadata",
        "Full text",
        "Notes",
        "Check methods",
        "q quit",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn compact_terminal_receives_actionable_message() {
    let mut state = snapshot();
    let text = render(&mut state, 35, 8);
    assert!(text.contains("48x12"), "{text}");
}

#[test]
#[ignore = "developer layout feedback; run with --ignored --nocapture"]
fn print_real_layout() {
    let mut state = snapshot();
    state.set_details(
        "a1".into(),
        Some(FullText {
            article_id: "a1".into(),
            file_path: "/articles/a1.pdf".into(),
            file_format: bib_base::FileFormat::Pdf,
            text_content: Some("Extracted paragraph.".into()),
            source: bib_base::FullTextSource::UserUpload,
            file_hash: None,
            file_size: Some(123),
            uploaded_at: None,
            extract_status: Some(bib_base::ExtractStatus::Done),
            text_format: Some(bib_base::TextFormat::Plain),
            extracted_by: Some("pdf".into()),
            extract_error: None,
        }),
        Vec::new(),
    );
    println!("--- 120x28 ---\n{}", render(&mut state, 120, 28));
    println!("--- 80x24 ---\n{}", render(&mut state, 80, 24));
}
