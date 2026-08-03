//! Integration tests for BibBase — SQLite round-trip via turso.

use bib_base::BibBase;
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

fn sample_article() -> Article {
    let mut art = Article::new("test-1", "A breakthrough paper");
    art.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("John A".into()),
        initials: Some("JA".into()),
        affiliation: Some("MIT".into()),
        orcid: Some("0000-0002-1825-0097".into()),
        corresponding: true,
    });
    art.authors.push(Author {
        last_name: "Jones".into(),
        fore_name: None,
        initials: Some("B".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    art.identifiers.push(Identifier::doi("10.1000/test"));
    art.identifiers.push(Identifier::pmid("12345"));
    art.abstract_text = Some("This is the abstract.".into());
    art.year = Some(2024);
    art.month = Some(3);
    art.journal = Some("Nature Genetics".into());
    art.volume = Some("56".into());
    art.issue = Some("3".into());
    art.pages = Some("100-110".into());
    art.issn = Some("1061-4036".into());
    art.language = Some("eng".into());
    art.pub_types = vec!["Journal Article".into()];
    art.keywords = vec!["Genetics".into(), "CRISPR".into()];
    art.source = ArticleSource::Pubmed;
    art
}

#[tokio::test]
async fn upsert_and_get_roundtrip() {
    let db = BibBase::open_in_memory().await.unwrap();
    let article = sample_article();

    db.upsert_article(&article).await.unwrap();

    let loaded = db
        .get_article("test-1")
        .await
        .unwrap()
        .expect("article not found");

    assert_eq!(loaded.id, "test-1");
    assert_eq!(loaded.title, "A breakthrough paper");
    assert_eq!(
        loaded.abstract_text.as_deref(),
        Some("This is the abstract.")
    );
    assert_eq!(loaded.year, Some(2024));
    assert_eq!(loaded.month, Some(3));
    assert_eq!(loaded.journal.as_deref(), Some("Nature Genetics"));
    assert_eq!(loaded.volume.as_deref(), Some("56"));
    assert_eq!(loaded.issue.as_deref(), Some("3"));
    assert_eq!(loaded.pages.as_deref(), Some("100-110"));
    assert_eq!(loaded.issn.as_deref(), Some("1061-4036"));
    assert_eq!(loaded.language.as_deref(), Some("eng"));
    assert_eq!(loaded.source, ArticleSource::Pubmed);
    assert_eq!(loaded.pub_types, vec!["Journal Article"]);
    assert_eq!(loaded.keywords, vec!["Genetics", "CRISPR"]);

    // Authors
    assert_eq!(loaded.authors.len(), 2);
    assert_eq!(loaded.authors[0].last_name, "Smith");
    assert_eq!(loaded.authors[0].fore_name.as_deref(), Some("John A"));
    assert_eq!(loaded.authors[0].initials.as_deref(), Some("JA"));
    assert_eq!(
        loaded.authors[0].orcid.as_deref(),
        Some("0000-0002-1825-0097")
    );
    assert!(loaded.authors[0].corresponding);
    assert_eq!(loaded.authors[1].last_name, "Jones");
    assert!(!loaded.authors[1].corresponding);

    // Identifiers
    assert_eq!(loaded.identifiers.len(), 2);
    assert_eq!(loaded.doi(), Some("10.1000/test"));
    assert_eq!(loaded.pmid(), Some("12345"));
}

#[tokio::test]
async fn find_by_doi() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_article(&sample_article()).await.unwrap();

    let found = db
        .find_by_identifier(IdKind::Doi, "10.1000/test")
        .await
        .unwrap()
        .expect("not found by DOI");
    assert_eq!(found.title, "A breakthrough paper");

    let found_pmid = db
        .find_by_identifier(IdKind::Pmid, "12345")
        .await
        .unwrap()
        .expect("not found by PMID");
    assert_eq!(found_pmid.id, "test-1");
}

#[tokio::test]
async fn find_by_doi_miss() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_article(&sample_article()).await.unwrap();

    let miss = db
        .find_by_identifier(IdKind::Doi, "10.9999/nonexistent")
        .await
        .unwrap();
    assert!(miss.is_none());
}

#[tokio::test]
async fn article_count() {
    let db = BibBase::open_in_memory().await.unwrap();
    assert_eq!(db.article_count().await.unwrap(), 0);

    db.upsert_article(&sample_article()).await.unwrap();
    assert_eq!(db.article_count().await.unwrap(), 1);

    // Upsert again — should replace, not duplicate.
    db.upsert_article(&sample_article()).await.unwrap();
    assert_eq!(db.article_count().await.unwrap(), 1);
}

#[tokio::test]
async fn delete_article() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_article(&sample_article()).await.unwrap();
    assert_eq!(db.article_count().await.unwrap(), 1);

    db.delete_article("test-1").await.unwrap();
    assert_eq!(db.article_count().await.unwrap(), 0);

    let gone = db.get_article("test-1").await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn upsert_replaces_child_rows() {
    let db = BibBase::open_in_memory().await.unwrap();

    // Insert with 2 authors + 2 identifiers.
    db.upsert_article(&sample_article()).await.unwrap();

    // Now upsert with fewer authors and identifiers.
    let mut art = sample_article();
    art.authors.truncate(1);
    art.identifiers.truncate(1);
    db.upsert_article(&art).await.unwrap();

    let loaded = db.get_article("test-1").await.unwrap().unwrap();
    assert_eq!(loaded.authors.len(), 1, "old authors should be replaced");
    assert_eq!(
        loaded.identifiers.len(),
        1,
        "old identifiers should be replaced"
    );
}
