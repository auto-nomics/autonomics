//! Article filtering used by the bibliography workspace.

use bib_base::Article;

pub(super) fn article_matches(article: &Article, needle: &str) -> bool {
    article.title.to_lowercase().contains(needle)
        || article
            .abstract_text
            .as_deref()
            .is_some_and(|text| text.to_lowercase().contains(needle))
        || article
            .journal
            .as_deref()
            .is_some_and(|text| text.to_lowercase().contains(needle))
        || article
            .keywords
            .iter()
            .any(|keyword| keyword.to_lowercase().contains(needle))
        || article
            .authors
            .iter()
            .any(|author| author.display_name().to_lowercase().contains(needle))
        || article.doi().is_some_and(|doi| doi.contains(needle))
}
