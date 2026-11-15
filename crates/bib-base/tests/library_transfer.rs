use bib_base::{BibBase, NewAttachment, article_revision};
use bib_types::{AddedBy, AnnotationKind, Article, ArticleRole, Collection, Identifier};
use std::collections::HashMap;

#[tokio::test]
async fn import_is_atomic_idempotent_and_never_overwrites() {
    let bib = BibBase::open_in_memory().await.unwrap();
    let mut a = Article::new("import-a", "Original");
    a.identifiers.push(Identifier::doi("10.1234/a"));
    let first = bib
        .import_library_records(vec![a.clone()], None)
        .await
        .unwrap();
    assert_eq!(first.imported_ids, vec!["import-a"]);
    let mut changed = a.clone();
    changed.title = "Should not overwrite".into();
    changed.id = "different-id".into();
    let second = bib
        .import_library_records(vec![changed], None)
        .await
        .unwrap();
    assert_eq!(second.skipped_ids, vec!["import-a"]);
    assert_eq!(
        bib.get_article("import-a").await.unwrap().unwrap().title,
        "Original"
    );
    assert!(
        bib.import_library_records(vec![Article::new("new", "New")], Some("missing".into()))
            .await
            .is_err()
    );
    assert!(bib.get_article("new").await.unwrap().is_none());
}
#[tokio::test]
async fn merge_preserves_attachment_keys_notes_membership_and_checks_revision() {
    let bib = BibBase::open_in_memory().await.unwrap();
    let mut target = Article::new("target", "Keep title");
    target.keywords = vec!["one".into()];
    let mut source = Article::new("source", "Other title");
    source.keywords = vec!["two".into()];
    source.abstract_text = Some("Selected source abstract".into());
    source
        .extra_fields
        .insert("publisher".into(), "Source Publisher".into());
    bib.upsert_articles(&[target.clone(), source.clone()])
        .await
        .unwrap();
    let collection = Collection::new("c", "Collection");
    bib.upsert_collection(&collection).await.unwrap();
    bib.add_to_collection("c", "source", ArticleRole::Referenced, AddedBy::User, None)
        .await
        .unwrap();
    let attachment = bib
        .create_zotero_attachment(
            NewAttachment {
                article_id: "source".into(),
                link_mode: 0,
                content_type: Some("text/plain".into()),
                path: Some("storage:notes.txt".into()),
                title: Some("Notes file".into()),
                source_url: None,
                filename: Some("notes.txt".into()),
                file_format: bib_types::FileFormat::Txt,
                vfs_path: Some("vfs:///bib/source/notes.txt".into()),
                storage_hash: None,
                file_size: Some(12),
            },
            true,
        )
        .await
        .unwrap();
    bib.record_attachment_extraction_failure(&attachment.key, "fixture has no extractor")
        .await
        .unwrap();
    let note = bib
        .add_annotation(
            "source",
            AnnotationKind::Comment,
            "Preserve note",
            None,
            None,
            None,
            Some(&attachment.key),
        )
        .await
        .unwrap();
    let mut expected = HashMap::from([
        ("target".into(), article_revision(&target)),
        ("source".into(), "stale".into()),
    ]);
    assert!(
        bib.merge_library_articles("target".into(), vec!["source".into()], expected.clone())
            .await
            .is_err()
    );
    assert!(bib.get_article("source").await.unwrap().is_some());
    expected.insert("source".into(), article_revision(&source));
    let mut chosen = target.clone();
    chosen.abstract_text = source.abstract_text.clone();
    let merged = bib
        .merge_library_articles_with_metadata(
            "target".into(),
            vec!["source".into()],
            expected,
            Some(chosen),
        )
        .await
        .unwrap();
    assert_eq!(merged.title, "Keep title");
    assert_eq!(
        merged.abstract_text.as_deref(),
        Some("Selected source abstract")
    );
    assert_eq!(
        merged.extra_fields.get("publisher").map(String::as_str),
        Some("Source Publisher")
    );
    assert_eq!(merged.keywords, vec!["one", "two"]);
    assert!(bib.get_article("source").await.unwrap().is_none());
    assert_eq!(
        bib.zotero_attachments("target").await.unwrap()[0].key,
        attachment.key
    );
    assert_eq!(
        bib.list_annotations("target", None).await.unwrap()[0].id,
        note.id
    );
    let collection = bib.get_collection("c").await.unwrap().unwrap();
    assert!(collection.article_ids.contains(&"target".into()));
    assert!(!collection.article_ids.contains(&"source".into()));
}

#[tokio::test]
async fn imports_preserve_case_sensitive_other_ids_and_reject_ambiguous_identity() {
    let bib = BibBase::open_in_memory().await.unwrap();
    let mut upper = Article::new("upper", "Upper path");
    upper.identifiers.push(Identifier::new(
        bib_types::IdKind::Other,
        "https://example.org/Paper",
    ));
    let mut lower = Article::new("lower", "Lower path");
    lower.identifiers.push(Identifier::new(
        bib_types::IdKind::Other,
        "https://example.org/paper",
    ));
    let result = bib
        .import_library_records(vec![upper, lower], None)
        .await
        .unwrap();
    assert_eq!(result.imported_ids.len(), 2);
    let mut a = Article::new("doi-owner", "DOI owner");
    a.identifiers
        .push(Identifier::doi("https://doi.org/10.1234/ABC"));
    let mut b = Article::new("pmid-owner", "PMID owner");
    b.identifiers.push(Identifier::pmid("12345"));
    bib.upsert_articles(&[a, b]).await.unwrap();
    let mut incoming = Article::new("incoming", "Conflicting identities");
    incoming.identifiers = vec![Identifier::doi("10.1234/abc"), Identifier::pmid("12345")];
    assert!(
        bib.import_library_records(vec![incoming], None)
            .await
            .is_err()
    );
    assert!(bib.get_article("incoming").await.unwrap().is_none());
}

#[tokio::test]
async fn merge_override_cannot_replace_identity_and_attachment_changes_abort() {
    let bib = BibBase::open_in_memory().await.unwrap();
    let mut target = Article::new("target", "Target");
    target.identifiers.push(Identifier::doi("10.1234/target"));
    let source = Article::new("source", "Source");
    let mut third = Article::new("third", "Third");
    third.identifiers.push(Identifier::doi("10.1234/third"));
    bib.upsert_articles(&[target.clone(), source.clone(), third.clone()])
        .await
        .unwrap();
    let expected = HashMap::from([
        ("target".into(), article_revision(&target)),
        ("source".into(), article_revision(&source)),
    ]);
    let assets = bib
        .library_merge_asset_revisions(vec!["source".into()])
        .await
        .unwrap();
    let attachment = bib
        .create_zotero_attachment(
            NewAttachment {
                article_id: "source".into(),
                title: None,
                filename: None,
                content_type: None,
                file_format: bib_types::FileFormat::Txt,
                link_mode: 3,
                path: None,
                source_url: Some("https://example.org".into()),
                vfs_path: None,
                storage_hash: None,
                file_size: None,
            },
            false,
        )
        .await
        .unwrap();
    bib.record_attachment_extraction_failure(&attachment.key, "linked fixture")
        .await
        .unwrap();
    let mut metadata = third;
    metadata.title = "Selected title".into();
    metadata.year = Some(0);
    assert!(
        bib.merge_library_articles_checked(
            "target".into(),
            vec!["source".into()],
            expected.clone(),
            Some(metadata.clone()),
            Some(assets.clone())
        )
        .await
        .is_err()
    );
    metadata.year = Some(2024);
    assert!(
        bib.merge_library_articles_checked(
            "target".into(),
            vec!["source".into()],
            expected.clone(),
            Some(metadata.clone()),
            Some(assets)
        )
        .await
        .is_err()
    );
    assert!(bib.get_article("source").await.unwrap().is_some());
    let merged = bib
        .merge_library_articles_with_metadata(
            "target".into(),
            vec!["source".into()],
            expected,
            Some(metadata),
        )
        .await
        .unwrap();
    assert_eq!(merged.id, "target");
    assert_eq!(merged.title, "Selected title");
    assert_eq!(merged.doi(), Some("10.1234/target"));
}
