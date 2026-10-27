use bib_base::{
    BibBase,
    collections::{CollectionChange, CollectionQuery},
};
use bib_types::{AddedBy, Article, ArticleRole, Collection};

async fn collection(db: &BibBase, id: &str, parent: Option<&str>) {
    let mut c = Collection::new(id, id);
    c.parent_id = parent.map(str::to_owned);
    db.upsert_collection(&c).await.unwrap();
}
fn query(id: Option<&str>, recursive: bool, unfiled: bool) -> CollectionQuery {
    CollectionQuery {
        collection_id: id.map(str::to_owned),
        include_descendants: recursive,
        unfiled,
        query: String::new(),
        limit: 100,
        offset: 0,
    }
}

#[tokio::test]
async fn nested_move_rejects_cycles_and_preserves_memberships() {
    let db = BibBase::open_in_memory().await.unwrap();
    collection(&db, "a", None).await;
    collection(&db, "b", Some("a")).await;
    collection(&db, "c", Some("b")).await;
    db.upsert_article(&Article::new("paper", "Nested paper"))
        .await
        .unwrap();
    db.add_to_collection(
        "c",
        "paper",
        ArticleRole::Cited,
        AddedBy::User,
        Some("keep me"),
    )
    .await
    .unwrap();
    for parent in ["a", "b", "c"] {
        assert!(
            db.change_collection(
                "a",
                CollectionChange::Update {
                    name: None,
                    parent_id: Some(Some(parent.into()))
                }
            )
            .await
            .is_err()
        );
    }
    assert!(
        db.change_collection(
            "b",
            CollectionChange::Update {
                name: None,
                parent_id: Some(Some("missing".into()))
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        db.get_collection("b")
            .await
            .unwrap()
            .unwrap()
            .parent_id
            .as_deref(),
        Some("a")
    );
    db.change_collection(
        "b",
        CollectionChange::Update {
            name: Some("Renamed".into()),
            parent_id: Some(None),
        },
    )
    .await
    .unwrap();
    let b = db.get_collection("b").await.unwrap().unwrap();
    assert_eq!(b.parent_id, None);
    assert_eq!(b.name, "Renamed");
    assert!(!b.key.is_empty());
    assert_eq!(
        db.get_collection("c")
            .await
            .unwrap()
            .unwrap()
            .parent_id
            .as_deref(),
        Some("b")
    );
    let ca = db.list_collection_articles("c", None, None).await.unwrap();
    assert_eq!(ca[0].role, ArticleRole::Cited);
    assert_eq!(ca[0].note.as_deref(), Some("keep me"));
    db.migrate().await.unwrap();
    assert_eq!(db.get_collection("b").await.unwrap().unwrap().key, b.key);
    assert_eq!(
        db.get_collection("c")
            .await
            .unwrap()
            .unwrap()
            .parent_id
            .as_deref(),
        Some("b")
    );
    // Metadata refresh by an older caller must not silently move a node.
    db.upsert_collection(&Collection::new("c", "Refresh"))
        .await
        .unwrap();
    assert_eq!(
        db.get_collection("c")
            .await
            .unwrap()
            .unwrap()
            .parent_id
            .as_deref(),
        Some("b")
    );
}

#[tokio::test]
async fn recursive_membership_deduplicates_without_losing_provenance_and_trash_restores() {
    let db = BibBase::open_in_memory().await.unwrap();
    collection(&db, "a", None).await;
    collection(&db, "b", Some("a")).await;
    collection(&db, "c", Some("b")).await;
    for id in ["shared", "child-only"] {
        db.upsert_article(&Article::new(id, id)).await.unwrap();
    }
    db.add_to_collection(
        "a",
        "shared",
        ArticleRole::Referenced,
        AddedBy::User,
        Some("parent note"),
    )
    .await
    .unwrap();
    db.add_to_collection(
        "c",
        "shared",
        ArticleRole::Cited,
        AddedBy::User,
        Some("child note"),
    )
    .await
    .unwrap();
    db.add_to_collection(
        "c",
        "child-only",
        ArticleRole::Background,
        AddedBy::User,
        None,
    )
    .await
    .unwrap();
    let direct = db
        .query_collection_articles(query(Some("a"), false, false))
        .await
        .unwrap();
    assert_eq!(direct.total, 1);
    let recursive = db
        .query_collection_articles(query(Some("a"), true, false))
        .await
        .unwrap();
    assert_eq!(recursive.total, 2);
    assert_eq!(recursive.articles.len(), 2);
    assert_eq!(recursive.associations.len(), 3);
    assert_eq!(
        db.query_collection_articles(query(None, false, true))
            .await
            .unwrap()
            .total,
        0
    );
    let affected = db
        .change_collection("b", CollectionChange::Trash)
        .await
        .unwrap();
    assert_eq!(affected.len(), 2);
    assert!(db.get_collection("b").await.unwrap().is_none());
    assert!(db.get_collection("c").await.unwrap().is_none());
    assert_eq!(db.list_collections(None).await.unwrap().len(), 1);
    assert_eq!(
        db.list_collections_including_deleted(None, true)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        db.query_collection_articles(query(None, false, true))
            .await
            .unwrap()
            .total,
        1
    );
    assert!(
        db.add_to_collection("b", "shared", ArticleRole::Cited, AddedBy::User, None)
            .await
            .is_err()
    );
    db.change_collection("c", CollectionChange::Restore)
        .await
        .unwrap();
    assert!(db.get_collection("b").await.unwrap().is_some());
    assert_eq!(
        db.list_collection_articles("c", None, None)
            .await
            .unwrap()
            .len(),
        2
    );
    db.change_collection("b", CollectionChange::Trash)
        .await
        .unwrap();
    db.change_collection("b", CollectionChange::Erase)
        .await
        .unwrap();
    assert_eq!(
        db.list_collections_including_deleted(None, true)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.article_count().await.unwrap(), 2);
    assert_eq!(
        db.list_collection_articles("a", None, None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn scoped_search_counts_and_pages_beyond_five_hundred() {
    let db = BibBase::open_in_memory().await.unwrap();
    collection(&db, "a", None).await;
    collection(&db, "b", Some("a")).await;
    let articles = (0..515)
        .map(|i| {
            let mut a = Article::new(format!("p{i:04}"), format!("Paper {i:04}"));
            a.abstract_text = Some("scopekeyword".into());
            a
        })
        .collect::<Vec<_>>();
    let assignments = articles
        .iter()
        .map(|a| bib_base::collections::CollectionAssignment {
            article_id: a.id.clone(),
            role: ArticleRole::Referenced,
            added_by: AddedBy::User,
            note: None,
        })
        .collect::<Vec<_>>();
    db.upsert_articles_with_collection(&articles, "b", &assignments)
        .await
        .unwrap();
    let mut q = query(Some("a"), true, false);
    q.query = "scopekeyword".into();
    q.limit = 500;
    let first = db.query_collection_articles(q.clone()).await.unwrap();
    q.offset = 500;
    let last = db.query_collection_articles(q).await.unwrap();
    assert_eq!(first.total, 515);
    assert_eq!(last.total, 515);
    assert_eq!(first.articles.len(), 500);
    assert_eq!(last.articles.len(), 15);
    assert!(
        !first
            .articles
            .iter()
            .any(|a| last.articles.iter().any(|b| a.id == b.id))
    );
    assert_eq!(
        db.query_collection_articles(query(Some("a"), false, false))
            .await
            .unwrap()
            .total,
        0
    );
}

#[tokio::test]
async fn collection_notifications_only_publish_successful_changes() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut changes = db.subscribe_collection_changes();
    let initial = *changes.borrow_and_update();
    collection(&db, "a", None).await;
    assert!(changes.has_changed().unwrap());
    assert!(*changes.borrow_and_update() > initial);
    assert!(
        db.change_collection(
            "a",
            CollectionChange::Update {
                name: None,
                parent_id: Some(Some("a".into()))
            }
        )
        .await
        .is_err()
    );
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn user_add_is_a_noop_for_existing_semantic_membership() {
    let db = BibBase::open_in_memory().await.unwrap();
    collection(&db, "a", None).await;
    db.upsert_article(&Article::new("paper", "Paper"))
        .await
        .unwrap();
    db.add_to_collection(
        "a",
        "paper",
        ArticleRole::Cited,
        AddedBy::Agent,
        Some("Research note"),
    )
    .await
    .unwrap();
    let before = db.list_collection_articles("a", None, None).await.unwrap();
    let outcome = db.ensure_collection_membership("a", "paper").await.unwrap();
    assert!(outcome.was_noop());
    assert_eq!(
        db.list_collection_articles("a", None, None).await.unwrap(),
        before
    );
    assert!(
        db.change_collection("a", CollectionChange::Erase)
            .await
            .is_err()
    );
    db.delete_collection("a").await.unwrap();
    assert!(
        db.list_collections_including_deleted(None, true)
            .await
            .unwrap()[0]
            .deleted
    );
}
