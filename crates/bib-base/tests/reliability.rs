use bib_base::{BibBase, SearchEngineMode, article_revision, collections::CollectionQuery};
use bib_types::{
    AddedBy, AnnotationKind, Article, ArticleRole, Collection, Identifier, SearchRequestV2,
};

#[tokio::test]
async fn trash_hides_search_lists_and_counts_but_restores_memberships_and_notes() {
    for engine in [SearchEngineMode::Tantivy, SearchEngineMode::Legacy] {
        let db = BibBase::open_with_search_engine(":memory:", engine)
            .await
            .unwrap();
        let mut article = Article::new("paper", "Unique genome paper");
        article.identifiers.push(Identifier::doi("10.1234/trash"));
        db.upsert_article(&article).await.unwrap();
        db.upsert_collection(&Collection::new("c", "Collection"))
            .await
            .unwrap();
        db.add_to_collection("c", "paper", ArticleRole::Referenced, AddedBy::User, None)
            .await
            .unwrap();
        let annotation = db
            .add_annotation(
                "paper",
                AnnotationKind::Comment,
                "Keep this note",
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(db.search_articles("genome", 10, 0).await.unwrap().len(), 1);
        assert!(db.set_article_trash("paper", true).await.unwrap());
        assert!(!db.set_article_trash("paper", true).await.unwrap());
        assert!(db.get_article("paper").await.unwrap().is_none());
        assert_eq!(db.article_count().await.unwrap(), 0);
        assert!(db.list_all_articles().await.unwrap().is_empty());
        assert!(
            db.search_articles("genome", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(db.search_articles("", 10, 0).await.unwrap().is_empty());
        if engine == SearchEngineMode::Tantivy {
            assert!(
                db.search_v2(SearchRequestV2 {
                    query: "genome".into(),
                    ..Default::default()
                })
                .await
                .unwrap()
                .hits
                .is_empty()
            );
        }
        let page = db
            .query_collection_articles(CollectionQuery {
                collection_id: Some("c".into()),
                include_descendants: true,
                unfiled: false,
                query: String::new(),
                limit: 50,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(page.total, 0);
        assert!(page.articles.is_empty());
        assert!(
            db.get_collection("c")
                .await
                .unwrap()
                .unwrap()
                .article_ids
                .is_empty()
        );
        let (trash, total) = db.list_article_trash(50, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(trash[0].article.title, article.title);
        assert_eq!(
            db.list_annotations("paper", None).await.unwrap()[0].id,
            annotation.id
        );
        assert!(db.set_article_trash("paper", false).await.unwrap());
        assert_eq!(db.article_count().await.unwrap(), 1);
        assert_eq!(
            db.get_collection("c").await.unwrap().unwrap().article_ids,
            vec!["paper"]
        );
        assert_eq!(db.search_articles("genome", 10, 0).await.unwrap().len(), 1);
        assert!(db.erase_trashed_article("paper").await.is_err());
        db.set_article_trash("paper", true).await.unwrap();
        assert_eq!(db.erase_trashed_article("paper").await.unwrap(), 1);
        assert_eq!(db.list_article_trash(50, 0).await.unwrap().1, 0);
        assert!(db.list_annotations("paper", None).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn stale_metadata_cannot_overwrite_agent_changes_or_restore_trashed_records() {
    let db = BibBase::open_in_memory().await.unwrap();
    let original = Article::new("paper", "Original");
    db.upsert_article(&original).await.unwrap();
    let before = db.get_article("paper").await.unwrap().unwrap();
    let revision = article_revision(&before);
    let mut agent = before.clone();
    agent.title = "Agent edit".into();
    db.upsert_article(&agent).await.unwrap();
    let mut user = before.clone();
    user.title = "Stale user edit".into();
    assert!(matches!(
        db.update_article_checked(user.clone(), Some(revision))
            .await,
        Err(bib_base::Error::MetadataConflict(_))
    ));
    assert_eq!(
        db.get_article("paper").await.unwrap().unwrap().title,
        "Agent edit"
    );
    let latest = db.get_article("paper").await.unwrap().unwrap();
    let saved = db
        .update_article_checked(user, Some(article_revision(&latest)))
        .await
        .unwrap();
    assert_eq!(saved.title, "Stale user edit");
    assert_eq!(saved.created_at, before.created_at);
    db.set_article_trash("paper", true).await.unwrap();
    assert!(matches!(
        db.update_article_checked(saved, None).await,
        Err(bib_base::Error::NotFound(_))
    ));
}

#[tokio::test]
async fn concurrent_editors_only_one_can_commit_the_same_revision() {
    let db = std::sync::Arc::new(BibBase::open_in_memory().await.unwrap());
    db.upsert_article(&Article::new("paper", "Original"))
        .await
        .unwrap();
    let original = db.get_article("paper").await.unwrap().unwrap();
    let revision = article_revision(&original);
    let mut first = original.clone();
    first.title = "First".into();
    let mut second = original;
    second.title = "Second".into();
    let (a, b) = tokio::join!(
        db.update_article_checked(first, Some(revision.clone())),
        db.update_article_checked(second, Some(revision))
    );
    assert_ne!(a.is_ok(), b.is_ok());
}

#[tokio::test]
async fn manual_creation_and_edits_reject_duplicate_identifiers_atomically() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut input = Article::new("ignored", "A new manual record");
    input
        .identifiers
        .push(Identifier::doi("10.1234/concurrent"));
    let (a, b) = tokio::join!(
        db.create_manual_article_checked(input.clone()),
        db.create_manual_article_checked(input)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let first = a.or(b).unwrap();
    let mut other = Article::new("other", "Other");
    db.upsert_article(&other).await.unwrap();
    other.identifiers = first.identifiers.clone();
    assert!(matches!(
        db.update_article_checked(other, None).await,
        Err(bib_base::Error::MetadataConflict(_))
    ));
    assert!(
        db.get_article("other")
            .await
            .unwrap()
            .unwrap()
            .identifiers
            .is_empty()
    );
}

#[tokio::test]
async fn trash_survives_database_reopen_and_search_rebuild() {
    let directory = std::env::temp_dir().join(format!("bib-trash-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("bib.db");
    {
        let db = BibBase::open(path.to_str().unwrap()).await.unwrap();
        db.upsert_article(&Article::new("paper", "Persistent genome"))
            .await
            .unwrap();
        db.set_article_trash("paper", true).await.unwrap();
    }
    {
        let db = BibBase::open(path.to_str().unwrap()).await.unwrap();
        db.rebuild_search_index().await.unwrap();
        assert_eq!(db.list_article_trash(50, 0).await.unwrap().1, 1);
        assert!(
            db.search_articles("genome", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        db.set_article_trash("paper", false).await.unwrap();
        assert_eq!(db.search_articles("genome", 10, 0).await.unwrap().len(), 1);
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn type_specific_metadata_roundtrips_in_details_lists_edits_and_trash() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut book = Article::new("book", "Book title");
    book.pub_types = vec!["Book".into()];
    book.extra_fields
        .insert("publisher".into(), "University Press".into());
    book.extra_fields.insert("edition".into(), "2".into());
    db.upsert_article(&book).await.unwrap();
    let detail = db.get_article("book").await.unwrap().unwrap();
    assert_eq!(detail.extra_fields, book.extra_fields);
    let listed = db
        .query_collection_articles(CollectionQuery {
            collection_id: None,
            include_descendants: false,
            unfiled: false,
            query: String::new(),
            limit: 50,
            offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(listed.articles[0].extra_fields, book.extra_fields);
    let revision = article_revision(&detail);
    let mut edited = detail;
    edited
        .extra_fields
        .insert("publisher".into(), "Revised Press".into());
    let saved = db
        .update_article_checked(edited, Some(revision.clone()))
        .await
        .unwrap();
    assert_ne!(article_revision(&saved), revision);
    db.set_article_trash("book", true).await.unwrap();
    assert_eq!(
        db.list_article_trash(50, 0).await.unwrap().0[0]
            .article
            .extra_fields,
        saved.extra_fields
    );
    db.set_article_trash("book", false).await.unwrap();
    assert_eq!(
        db.get_article("book").await.unwrap().unwrap().extra_fields,
        saved.extra_fields
    );
}

#[tokio::test]
async fn manual_identifiers_preserve_case_sensitive_url_paths() {
    let db = BibBase::open_in_memory().await.unwrap();
    for value in ["https://example.org/Paper", "https://example.org/paper"] {
        let mut article = Article::new("", "Web page");
        article
            .identifiers
            .push(Identifier::new(bib_types::IdKind::Other, value));
        db.create_manual_article_checked(article).await.unwrap();
    }
    assert_eq!(db.article_count().await.unwrap(), 2);
    let mut first = Article::new("", "Paper");
    first.identifiers.push(Identifier::doi("10.1234/AbC"));
    db.create_manual_article_checked(first.clone())
        .await
        .unwrap();
    first.identifiers[0].value = "10.1234/abc".into();
    assert!(matches!(
        db.create_manual_article_checked(first).await,
        Err(bib_base::Error::MetadataConflict(_))
    ));
}
