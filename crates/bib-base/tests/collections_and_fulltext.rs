//! Integration tests for collections, collection-article associations,
//! full-text storage, and FTS search.

use bib_base::BibBase;
use bib_types::{
    AddedBy, Article, ArticleRole, ArticleSource, Author, Collection, CollectionStatus,
    FetchStatus, FileFormat, FullText, FullTextSource, IdKind, Identifier,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_article(id: &str, title: &str) -> Article {
    let mut art = Article::new(id, title);
    art.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("John A".into()),
        initials: Some("JA".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    art.identifiers
        .push(Identifier::doi(format!("10.1000/{id}")));
    art.identifiers.push(Identifier::pmid(format!("100{id}")));
    art.abstract_text = Some("A paper about CRISPR gene editing and off-target effects.".into());
    art.year = Some(2024);
    art.journal = Some("Nature Genetics".into());
    art.keywords = vec!["CRISPR".into(), "gene editing".into()];
    art.source = ArticleSource::Pubmed;
    art
}

// ---------------------------------------------------------------------------
// Collection CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn collection_crud() {
    let db = BibBase::open_in_memory().await.unwrap();

    let mut col = Collection::new("col-1", "eQTL colocalization");
    col.description = Some("Investigation of colocalization methods".into());
    col.tags = vec!["eQTL".into(), "coloc".into()];
    db.upsert_collection(&col).await.unwrap();

    let loaded = db
        .get_collection("col-1")
        .await
        .unwrap()
        .expect("not found");
    assert_eq!(loaded.name, "eQTL colocalization");
    assert_eq!(
        loaded.description.as_deref(),
        Some("Investigation of colocalization methods")
    );
    assert_eq!(loaded.tags, vec!["eQTL", "coloc"]);
    assert_eq!(loaded.status, CollectionStatus::Active);
    assert!(loaded.article_ids.is_empty());

    // Update status.
    db.update_collection_status("col-1", CollectionStatus::Completed)
        .await
        .unwrap();
    let loaded = db.get_collection("col-1").await.unwrap().unwrap();
    assert_eq!(loaded.status, CollectionStatus::Completed);
}

#[tokio::test]
async fn collection_list_with_filter() {
    let db = BibBase::open_in_memory().await.unwrap();

    let c1 = Collection::new("c1", "Active project");
    let mut c2 = Collection::new("c2", "Finished project");
    c2.status = CollectionStatus::Completed;
    let mut c3 = Collection::new("c3", "Archived");
    c3.status = CollectionStatus::Archived;

    db.upsert_collection(&c1).await.unwrap();
    db.upsert_collection(&c2).await.unwrap();
    db.upsert_collection(&c3).await.unwrap();

    // All collections.
    let all = db.list_collections(None).await.unwrap();
    assert_eq!(all.len(), 3);

    // Only active.
    let active = db
        .list_collections(Some(CollectionStatus::Active))
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "c1");

    // Only completed.
    let completed = db
        .list_collections(Some(CollectionStatus::Completed))
        .await
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].id, "c2");
}

#[tokio::test]
async fn collection_delete_cascades() {
    let db = BibBase::open_in_memory().await.unwrap();

    db.upsert_collection(&Collection::new("c1", "Test"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a1", "Paper"))
        .await
        .unwrap();
    db.add_to_collection("c1", "a1", ArticleRole::Requested, AddedBy::Agent, None)
        .await
        .unwrap();

    db.delete_collection("c1").await.unwrap();

    assert!(db.get_collection("c1").await.unwrap().is_none());
    // Article should still exist.
    assert!(db.get_article("a1").await.unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Collection-article association
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_and_list_collection_articles() {
    let db = BibBase::open_in_memory().await.unwrap();

    db.upsert_collection(&Collection::new("c1", "Investigation"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a1", "Paper one"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a2", "Paper two"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a3", "Background paper"))
        .await
        .unwrap();

    db.add_to_collection(
        "c1",
        "a1",
        ArticleRole::Requested,
        AddedBy::Agent,
        Some("Key method"),
    )
    .await
    .unwrap();
    db.add_to_collection("c1", "a2", ArticleRole::Referenced, AddedBy::Agent, None)
        .await
        .unwrap();
    db.add_to_collection("c1", "a3", ArticleRole::Background, AddedBy::User, None)
        .await
        .unwrap();

    let articles = db.list_collection_articles("c1", None, None).await.unwrap();
    assert_eq!(articles.len(), 3);
    assert_eq!(articles[0].article_id, "a1");
    assert_eq!(articles[0].position, 0);
    assert_eq!(articles[0].role, ArticleRole::Requested);
    assert_eq!(articles[0].note.as_deref(), Some("Key method"));
    assert_eq!(articles[1].position, 1);
    assert_eq!(articles[2].role, ArticleRole::Background);
    assert_eq!(articles[2].added_by, AddedBy::User);

    // Filter by role.
    let requested = db
        .list_collection_articles("c1", Some(ArticleRole::Requested), None)
        .await
        .unwrap();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].article_id, "a1");

    // Update role.
    db.update_article_role("c1", "a1", ArticleRole::Cited)
        .await
        .unwrap();
    let updated = db
        .list_collection_articles("c1", Some(ArticleRole::Cited), None)
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);

    // Remove one.
    db.remove_from_collection("c1", "a2").await.unwrap();
    let remaining = db.list_collection_articles("c1", None, None).await.unwrap();
    assert_eq!(remaining.len(), 2);
}

#[tokio::test]
async fn add_to_collection_is_idempotent() {
    let db = BibBase::open_in_memory().await.unwrap();

    db.upsert_collection(&Collection::new("c1", "Test"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a1", "Paper"))
        .await
        .unwrap();

    db.add_to_collection(
        "c1",
        "a1",
        ArticleRole::Requested,
        AddedBy::Agent,
        Some("first note"),
    )
    .await
    .unwrap();
    db.add_to_collection(
        "c1",
        "a1",
        ArticleRole::Cited,
        AddedBy::User,
        Some("updated note"),
    )
    .await
    .unwrap();

    let articles = db.list_collection_articles("c1", None, None).await.unwrap();
    assert_eq!(articles.len(), 1, "should not duplicate");
    assert_eq!(articles[0].role, ArticleRole::Cited, "role should update");
    assert_eq!(
        articles[0].added_by,
        AddedBy::User,
        "added_by should update"
    );
    assert_eq!(articles[0].note.as_deref(), Some("updated note"));
    assert_eq!(articles[0].position, 0, "position should not change");
}

#[tokio::test]
async fn collection_article_ids_hydrated() {
    let db = BibBase::open_in_memory().await.unwrap();

    db.upsert_collection(&Collection::new("c1", "Test"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a1", "First"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a2", "Second"))
        .await
        .unwrap();

    db.add_to_collection("c1", "a1", ArticleRole::Referenced, AddedBy::Agent, None)
        .await
        .unwrap();
    db.add_to_collection("c1", "a2", ArticleRole::Referenced, AddedBy::Agent, None)
        .await
        .unwrap();

    let col = db.get_collection("c1").await.unwrap().unwrap();
    assert_eq!(col.article_ids, vec!["a1", "a2"]);
}

// ---------------------------------------------------------------------------
// Full-text requests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fulltext_request_lifecycle() {
    let db = BibBase::open_in_memory().await.unwrap();

    db.upsert_collection(&Collection::new("c1", "Investigation"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a1", "Needs full text"))
        .await
        .unwrap();
    db.upsert_article(&sample_article("a2", "Also needs full text"))
        .await
        .unwrap();

    db.add_to_collection("c1", "a1", ArticleRole::Requested, AddedBy::Agent, None)
        .await
        .unwrap();
    db.add_to_collection("c1", "a2", ArticleRole::Referenced, AddedBy::Agent, None)
        .await
        .unwrap();

    // No requests initially.
    let reqs = db.list_fulltext_requests(None).await.unwrap();
    assert!(reqs.is_empty());

    // Mark a1 as needing full text.
    db.update_fetch_status("c1", "a1", FetchStatus::FulltextRequested)
        .await
        .unwrap();

    let reqs = db.list_fulltext_requests(None).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].article_id, "a1");

    // Filter by collection.
    let reqs_c1 = db.list_fulltext_requests(Some("c1")).await.unwrap();
    assert_eq!(reqs_c1.len(), 1);

    // Mark a2 as well.
    db.update_fetch_status("c1", "a2", FetchStatus::FulltextRequested)
        .await
        .unwrap();
    assert_eq!(db.list_fulltext_requests(None).await.unwrap().len(), 2);

    // Fulfill a1.
    db.update_fetch_status("c1", "a1", FetchStatus::FulltextAvailable)
        .await
        .unwrap();
    let reqs = db.list_fulltext_requests(None).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].article_id, "a2");
}

// ---------------------------------------------------------------------------
// Full-text storage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fulltext_crud() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_article(&sample_article("a1", "Paper with full text"))
        .await
        .unwrap();

    assert!(!db.has_fulltext("a1").await.unwrap());

    let ft = FullText {
        article_id: "a1".into(),
        file_path: "/articles/a1.pdf".into(),
        file_format: FileFormat::Pdf,
        text_content: Some("The full text body of the paper about CRISPR.".into()),
        source: FullTextSource::UserUpload,
        file_hash: Some("sha256:abc123".into()),
        file_size: Some(1048576),
        uploaded_at: Some(chrono::Utc::now()),
    };
    db.upsert_fulltext(&ft).await.unwrap();

    assert!(db.has_fulltext("a1").await.unwrap());

    let loaded = db.get_fulltext("a1").await.unwrap().expect("not found");
    assert_eq!(loaded.file_path, "/articles/a1.pdf");
    assert_eq!(loaded.file_format, FileFormat::Pdf);
    assert_eq!(loaded.source, FullTextSource::UserUpload);
    assert_eq!(loaded.file_size, Some(1048576));
    assert_eq!(loaded.file_hash.as_deref(), Some("sha256:abc123"));

    // Delete.
    db.delete_fulltext("a1").await.unwrap();
    assert!(!db.has_fulltext("a1").await.unwrap());
    assert!(db.get_fulltext("a1").await.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Search (LIKE-based)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_basic() {
    let db = BibBase::open_in_memory().await.unwrap();

    let mut a1 = sample_article("a1", "CRISPR gene editing review");
    a1.abstract_text = Some("This review covers CRISPR-Cas9 off-target effects.".into());

    let mut a2 = sample_article("a2", "GWAS analysis of height");
    a2.abstract_text = Some("We performed a genome-wide association study.".into());

    let mut a3 = sample_article("a3", "CRISPR off-target detection methods");
    a3.abstract_text = Some("Novel methods for detecting CRISPR off-target cleavage.".into());

    db.upsert_article(&a1).await.unwrap();
    db.upsert_article(&a2).await.unwrap();
    db.upsert_article(&a3).await.unwrap();

    // Search for "CRISPR".
    let hits = db.search_articles("CRISPR", 10).await.unwrap();
    assert!(
        hits.len() >= 2,
        "should find at least 2 CRISPR articles, got {}",
        hits.len()
    );

    // All hits should mention CRISPR in title or snippet.
    for hit in &hits {
        let combined = format!("{} {}", hit.title, hit.snippet).to_lowercase();
        assert!(
            combined.contains("crispr"),
            "hit '{}' should mention CRISPR",
            hit.article_id
        );
    }

    // GWAS article should not be in CRISPR results.
    assert!(
        !hits.iter().any(|h| h.article_id == "a2"),
        "GWAS article should not match CRISPR query"
    );
}

#[tokio::test]
async fn search_with_fulltext() {
    let db = BibBase::open_in_memory().await.unwrap();

    let mut a1 = sample_article("a1", "Machine learning in genomics");
    a1.abstract_text = Some("An overview of ML applications.".into());
    db.upsert_article(&a1).await.unwrap();

    // Initially, searching for content only in full text should miss.
    let hits = db
        .search_articles("deep neural network architecture", 10)
        .await
        .unwrap();
    assert!(hits.is_empty());

    // Upload full text with the relevant content.
    let ft = FullText {
        article_id: "a1".into(),
        file_path: "/articles/a1.pdf".into(),
        file_format: FileFormat::Pdf,
        text_content: Some(
            "In section 3 we describe a deep neural network architecture \
             for predicting regulatory variants."
                .into(),
        ),
        source: FullTextSource::UserUpload,
        file_hash: None,
        file_size: None,
        uploaded_at: None,
    };
    db.upsert_fulltext(&ft).await.unwrap();

    // Now the full text content is searchable.
    let hits = db
        .search_articles("deep neural network architecture", 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].article_id, "a1");
    // Snippet should contain the matched text.
    assert!(
        hits[0]
            .snippet
            .to_lowercase()
            .contains("deep neural network")
    );
}

#[tokio::test]
async fn search_title_ranks_first() {
    let db = BibBase::open_in_memory().await.unwrap();

    // a1 has query in abstract only, a2 has it in title.
    let mut a1 = sample_article("a1", "General genomics");
    a1.abstract_text = Some("Discussion of proteomics methods in depth.".into());

    let mut a2 = sample_article("a2", "Proteomics breakthrough");
    a2.abstract_text = Some("We discovered a novel protein.".into());

    db.upsert_article(&a1).await.unwrap();
    db.upsert_article(&a2).await.unwrap();

    let hits = db.search_articles("proteomics", 10).await.unwrap();
    assert_eq!(hits.len(), 2);
    // Title match (rank 0) should come first.
    assert_eq!(hits[0].article_id, "a2");
    assert_eq!(hits[0].score, 0.0);
    // Abstract match (rank 1) second.
    assert_eq!(hits[1].article_id, "a1");
    assert_eq!(hits[1].score, 1.0);
}
