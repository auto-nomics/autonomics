//! Schema migration and paged-listing tests.
//!
//! The migration tests build a database with the **pre-`parent_id` /
//! pre-`data`** schema by hand, because `BibBase::open` always creates the
//! current schema — exercising the upgrade path needs an old file to
//! upgrade.

use std::path::PathBuf;

use bib_base::{BibBase, ListParams, SortField, SortOrder};
use bib_types::{AnnotationKind, Article, ArticleSource, Collection, Identifier};

/// The schema as it looked before `collections.parent_id`,
/// `collections.sort_order`, and `annotations.data` were introduced.
const LEGACY_SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS articles (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    abstract    TEXT,
    year        INTEGER,
    month       INTEGER,
    journal     TEXT,
    volume      TEXT,
    issue       TEXT,
    pages       TEXT,
    issn        TEXT,
    essn        TEXT,
    language    TEXT,
    pub_types   TEXT,
    keywords    TEXT,
    source      TEXT,
    created_at  TEXT,
    updated_at  TEXT
);
CREATE TABLE IF NOT EXISTS authors (
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    last_name   TEXT NOT NULL,
    fore_name   TEXT,
    initials    TEXT,
    affiliation TEXT,
    orcid       TEXT,
    corresponding INTEGER DEFAULT 0,
    PRIMARY KEY (article_id, position)
);
CREATE TABLE IF NOT EXISTS identifiers (
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (article_id, kind)
);
CREATE TABLE IF NOT EXISTS annotations (
    id          TEXT PRIMARY KEY,
    article_id  TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    content     TEXT NOT NULL,
    page        INTEGER,
    created_at  TEXT
);
CREATE TABLE IF NOT EXISTS collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    tags        TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT,
    updated_at  TEXT
);
CREATE TABLE IF NOT EXISTS collection_articles (
    collection_id  TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    article_id     TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    position       INTEGER NOT NULL,
    role           TEXT NOT NULL DEFAULT 'referenced',
    fetch_status   TEXT NOT NULL DEFAULT 'metadata_only',
    added_by       TEXT NOT NULL DEFAULT 'agent',
    note           TEXT,
    added_at       TEXT,
    PRIMARY KEY (collection_id, article_id)
);
CREATE TABLE IF NOT EXISTS fulltexts (
    article_id   TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    file_format  TEXT NOT NULL,
    text_content TEXT,
    source       TEXT NOT NULL,
    file_hash    TEXT,
    file_size    INTEGER,
    uploaded_at  TEXT
);
CREATE TABLE IF NOT EXISTS search_terms (
    term       TEXT NOT NULL,
    article_id TEXT NOT NULL,
    field      TEXT NOT NULL,
    PRIMARY KEY (term, article_id, field)
);
CREATE TABLE IF NOT EXISTS bib_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// A unique file per test run; turso needs a real file here (`:memory:`
/// would defeat the purpose of testing a reopen).
fn temp_db_path(label: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bib-base-{}-{label}.db",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[tokio::test]
async fn legacy_database_is_upgraded_in_place() {
    let path = temp_db_path("legacy");

    {
        let db = turso::Builder::new_local(path.to_string_lossy().as_ref())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(LEGACY_SCHEMA_SQL).await.unwrap();

        conn.execute(
            "INSERT INTO articles (id, title, source) VALUES ('a1', 'Legacy article', '\"manual\"')",
            turso::params![],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO annotations (id, article_id, kind, content, page, created_at) \
             VALUES ('ann-1', 'a1', 'highlight', 'keep me', 3, '2024-01-01T00:00:00Z')",
            turso::params![],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO collections (id, name, description, tags, status, created_at, updated_at) \
             VALUES ('col-1', 'Legacy collection', 'desc', '[]', 'active', \
                     '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            turso::params![],
        )
        .await
        .unwrap();
    }

    let db = BibBase::open(path.to_string_lossy().as_ref()).await.unwrap();

    // New columns exist…
    assert!(has_column_marker(&path, "collections", "parent_id").await);
    assert!(has_column_marker(&path, "collections", "sort_order").await);
    assert!(has_column_marker(&path, "annotations", "data").await);

    // …and the rows written with the old schema survive.
    let collection = db
        .get_collection("col-1")
        .await
        .unwrap()
        .expect("legacy collection must survive the migration");
    assert_eq!(collection.name, "Legacy collection");
    assert_eq!(collection.description.as_deref(), Some("desc"));
    // Absent data reads as the documented defaults.
    assert_eq!(collection.parent_id, None);
    assert_eq!(collection.sort_order, 0);

    let annotations = db.list_annotations("a1").await.unwrap();
    assert_eq!(annotations.len(), 1, "legacy annotation must survive");
    assert_eq!(annotations[0].content, "keep me");
    assert_eq!(annotations[0].page, Some(3));
    assert_eq!(annotations[0].data, None);

    // The upgraded database stays writable through the new fields.
    let mut collection = collection;
    collection.parent_id = None;
    collection.sort_order = 4;
    db.upsert_collection(&collection).await.unwrap();
    let reloaded = db.get_collection("col-1").await.unwrap().unwrap();
    assert_eq!(reloaded.sort_order, 4);

    let _ = std::fs::remove_file(&path);
}

/// Open a second handle on the same file and ask whether `column` exists.
///
/// `BibBase` keeps its connections private, and a fresh handle on the same
/// path sees the migrated schema, which is exactly what this test wants to
/// assert.
async fn has_column_marker(path: &std::path::Path, table: &str, column: &str) -> bool {
    let db = turso::Builder::new_local(path.to_string_lossy().as_ref())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), turso::params![])
        .await
        .unwrap();
    let mut found = false;
    while let Ok(Some(row)) = rows.next().await {
        if row.get::<String>(1).unwrap() == column {
            found = true;
            break;
        }
    }
    found
}

#[tokio::test]
async fn fresh_database_has_the_new_columns() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut collection = Collection::new("col-1", "Tree root");
    collection.parent_id = None;
    collection.sort_order = -3;
    db.upsert_collection(&collection).await.unwrap();
    assert_eq!(db.get_collection("col-1").await.unwrap().unwrap().sort_order, -3);

    let article = sample_article("a1", "Annotated");
    db.upsert_article(&article).await.unwrap();
    let annotation = db
        .add_annotation(
            "a1",
            AnnotationKind::Highlight,
            "with geometry",
            Some(2),
            Some(serde_json::json!({"rects": [1, 2]})),
        )
        .await
        .unwrap();
    let stored = db.list_annotations("a1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].data, annotation.data);
    assert_eq!(stored[0].data.as_ref().unwrap()["rects"].as_array().unwrap().len(), 2);
}

fn sample_article(id: &str, title: &str) -> Article {
    let mut article = Article::new(id, title);
    article.identifiers.push(Identifier::doi(format!("10.1000/{id}")));
    article.source = ArticleSource::Manual;
    article
}

// ---------------------------------------------------------------------------
// list_articles_paged
// ---------------------------------------------------------------------------

async fn seeded_db() -> BibBase {
    let db = BibBase::open_in_memory().await.unwrap();
    for (id, title, year) in [
        ("a1", "alpha", 2021),
        ("a2", "Beta", 2023),
        ("a3", "gamma", 2022),
        ("a4", "delta", 2023),
        ("a5", "epsilon", 2020),
    ] {
        let mut article = sample_article(id, title);
        article.year = Some(year);
        // Distinct, increasing timestamps so `created_at` ordering is
        // defined without relying on the tiebreaker.
        let stamp = chrono::DateTime::parse_from_rfc3339(&format!("2024-01-0{}T00:00:00Z", id.chars().last().unwrap().to_digit(10).unwrap()))
            .unwrap()
            .with_timezone(&chrono::Utc);
        article.created_at = Some(stamp);
        article.updated_at = Some(stamp);
        db.upsert_article(&article).await.unwrap();
    }
    db
}

/// Run a listing and reduce it to the ordered article IDs plus the total.
async fn ids(db: &BibBase, params: ListParams) -> (Vec<String>, usize) {
    let (articles, total) = db.list_articles_paged(&params).await.unwrap();
    (
        articles.into_iter().map(|article| article.id).collect(),
        total,
    )
}

#[tokio::test]
async fn paged_listing_sorts_by_whitelisted_keys() {
    let db = seeded_db().await;

    let (got, total) = ids(
        &db,
        ListParams {
            sort: SortField::Title,
            order: SortOrder::Ascending,
            ..Default::default()
        },
    )
    .await;
    // Case-folded, so "Beta" sorts next to "alpha" rather than after "z".
    assert_eq!(got, vec!["a1", "a2", "a4", "a5", "a3"]);
    assert_eq!(total, 5);

    let (got, _) = ids(
        &db,
        ListParams {
            sort: SortField::Title,
            order: SortOrder::Descending,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(got, vec!["a3", "a5", "a4", "a2", "a1"]);

    let (got, _) = ids(
        &db,
        ListParams {
            sort: SortField::Year,
            order: SortOrder::Descending,
            ..Default::default()
        },
    )
    .await;
    // The 2023 tie breaks on ID, reversed along with the ordering itself.
    assert_eq!(got, vec!["a4", "a2", "a3", "a1", "a5"]);

    let (got, _) = ids(
        &db,
        ListParams {
            sort: SortField::CreatedAt,
            order: SortOrder::Ascending,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(got, vec!["a1", "a2", "a3", "a4", "a5"]);

    // Offset/limit slice the sorted list, and `total` stays the full count.
    let (got, total) = ids(
        &db,
        ListParams {
            sort: SortField::CreatedAt,
            order: SortOrder::Ascending,
            offset: 2,
            limit: 2,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(got, vec!["a3", "a4"]);
    assert_eq!(total, 5);

    // An offset past the end is an empty page, not an error.
    let (got, total) = ids(
        &db,
        ListParams {
            offset: 50,
            limit: 2,
            ..Default::default()
        },
    )
    .await;
    assert!(got.is_empty());
    assert_eq!(total, 5);
}

#[tokio::test]
async fn paged_listing_filters_by_collection_and_unfiled() {
    let db = seeded_db().await;

    let mut collection = Collection::new("col-1", "Filed");
    db.upsert_collection(&collection).await.unwrap();
    for id in ["a2", "a4"] {
        db.add_to_collection("col-1", id, bib_types::ArticleRole::Referenced, bib_types::AddedBy::User, None)
            .await
            .unwrap();
    }
    collection.parent_id = Some("col-1".to_owned());
    db.upsert_collection(&collection).await.unwrap();

    let params = |collection_id: Option<&str>, unfiled: bool| ListParams {
        collection_id: collection_id.map(str::to_owned),
        unfiled,
        sort: SortField::CreatedAt,
        order: SortOrder::Ascending,
        limit: 100,
        ..Default::default()
    };

    let (articles, total) = db
        .list_articles_paged(&params(Some("col-1"), false))
        .await
        .unwrap();
    let got = articles.into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert_eq!(got, vec!["a2", "a4"], "membership order follows position");
    assert_eq!(total, 2);

    let (articles, total) = db
        .list_articles_paged(&params(None, true))
        .await
        .unwrap();
    let got = articles.into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert_eq!(got, vec!["a1", "a3", "a5"]);
    assert_eq!(total, 3);

    // Unknown collection → empty page with a zero total, not an error.
    let (articles, total) = db
        .list_articles_paged(&params(Some("col-none"), false))
        .await
        .unwrap();
    assert!(articles.is_empty());
    assert_eq!(total, 0);
}

#[tokio::test]
async fn paged_listing_applies_keyword_query_before_filters() {
    let db = seeded_db().await;

    // The query narrows first; the collection filter then intersects.
    let collection = Collection::new("col-1", "Filed");
    db.upsert_collection(&collection).await.unwrap();
    db.add_to_collection("col-1", "a1", bib_types::ArticleRole::Referenced, bib_types::AddedBy::User, None)
        .await
        .unwrap();

    let (articles, total) = db
        .list_articles_paged(&ListParams {
            query: "alpha".to_owned(),
            sort: SortField::Title,
            order: SortOrder::Ascending,
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    let got = articles.into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert_eq!(got, vec!["a1"]);
    assert_eq!(total, 1);

    let (articles, total) = db
        .list_articles_paged(&ListParams {
            query: "alpha".to_owned(),
            collection_id: Some("col-1".to_owned()),
            sort: SortField::Title,
            order: SortOrder::Ascending,
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(articles.len(), 1);
    assert_eq!(total, 1);

    // A query matching nothing filed elsewhere yields an empty page.
    let (articles, total) = db
        .list_articles_paged(&ListParams {
            query: "epsilon".to_owned(),
            collection_id: Some("col-1".to_owned()),
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(articles.is_empty());
    assert_eq!(total, 0);
}

// ---------------------------------------------------------------------------
// bib_meta
// ---------------------------------------------------------------------------

#[tokio::test]
async fn meta_store_round_trips_and_scans_prefixes() {
    let db = BibBase::open_in_memory().await.unwrap();

    assert_eq!(db.get_meta("web:missing").await.unwrap(), None);

    db.set_meta("web:a", "1").await.unwrap();
    db.set_meta("web:b", "2").await.unwrap();
    // A differently prefixed key and a bare key must stay out of the scan.
    db.set_meta("other:a", "3").await.unwrap();
    db.set_meta("a", "4").await.unwrap();

    let entries = db.list_meta_prefixed("web:").await.unwrap();
    assert_eq!(
        entries,
        vec![
            ("web:a".to_owned(), "1".to_owned()),
            ("web:b".to_owned(), "2".to_owned()),
        ]
    );

    // Upsert overwrites in place.
    db.set_meta("web:a", "9").await.unwrap();
    assert_eq!(db.get_meta("web:a").await.unwrap().as_deref(), Some("9"));

    // An empty prefix scans everything, including the search-index version
    // marker that `migrate` writes on every open.
    let everything = db.list_meta_prefixed("").await.unwrap();
    for key in ["web:a", "web:b", "other:a", "a", "search_index_version"] {
        assert!(
            everything.iter().any(|(stored, _)| stored == key),
            "empty prefix scan is missing {key}"
        );
    }
}
