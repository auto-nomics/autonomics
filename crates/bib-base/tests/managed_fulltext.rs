use bib_base::{
    BibShared,
    managed_fulltext::{materialize_legacy, repair_legacy_open_access, store_pdf},
    pmc_pdf::PmcPdf,
};
use bib_types::{
    AnnotationKind, Article, ExtractStatus, FileFormat, FullText, FullTextSource, TextFormat,
};
use std::sync::Arc;
use vfs::OpendalFileStorage;

fn legacy(article: &str, text: &str) -> FullText {
    FullText {
        article_id: article.into(),
        attachment_key: None,
        file_path: "europepmc:PMC42".into(),
        file_format: FileFormat::Txt,
        text_content: Some(text.into()),
        source: FullTextSource::OpenAccess,
        file_hash: None,
        file_size: Some(999999),
        uploaded_at: None,
        extract_status: Some(ExtractStatus::Done),
        text_format: Some(TextFormat::Plain),
        extracted_by: Some("europepmc".into()),
        extract_error: None,
    }
}
fn pdf() -> PmcPdf {
    PmcPdf {
        pmcid: "PMC42".into(),
        version: 1,
        source_url: "https://pmc-oa-opendata.s3.amazonaws.com/PMC42.1/PMC42.1.pdf".into(),
        bytes: b"%PDF-1.7\noriginal PDF bytes\n%%EOF".to_vec(),
    }
}

#[tokio::test]
async fn repairs_inline_text_without_replacing_attachment_identity_or_annotations() {
    let storage = Arc::new(OpendalFileStorage::new_temp());
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(storage.clone());
    shared
        .bib
        .upsert_article(&Article::new("paper", "Paper"))
        .await
        .unwrap();
    let text = "PMC text — 文本 α\nKeep paragraphs\n";
    shared
        .bib
        .upsert_fulltext(&legacy("paper", text))
        .await
        .unwrap();
    let before = shared.bib.get_fulltext("paper").await.unwrap().unwrap();
    let key = before.attachment_key.clone().unwrap();
    let note = shared
        .bib
        .add_annotation(
            "paper",
            AnnotationKind::Note,
            "Keep this note",
            None,
            None,
            None,
            Some(&key),
        )
        .await
        .unwrap();
    let repaired = materialize_legacy(&shared, before).await.unwrap();
    assert!(repaired.file_path.contains(&format!("/attachments/{key}/")));
    assert!(!repaired.file_path.contains("/extracted/"));
    assert_eq!(repaired.file_size, Some(text.len() as i64));
    assert_eq!(repaired.attachment_key.as_deref(), Some(key.as_str()));
    let bytes = bib_base::read_full(
        &storage,
        &bib_base::vfs_virtual_path(&repaired.file_path).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(bytes.as_slice(), text.as_bytes());
    let attachment = shared
        .bib
        .zotero_attachment("paper")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        attachment.link_mode,
        bib_base::zotero_model::LINK_MODE_IMPORTED_FILE
    );
    assert_eq!(attachment.original_filename.as_deref(), Some("PMC42.txt"));
    assert!(attachment.source_url.unwrap().starts_with("https://"));
    assert_eq!(
        shared.bib.list_annotations("paper", None).await.unwrap()[0].id,
        note.id
    );
    assert_eq!(repair_legacy_open_access(&shared).await.unwrap(), 0);
    assert_eq!(
        materialize_legacy(&shared, repaired.clone())
            .await
            .unwrap()
            .file_path,
        repaired.file_path
    );
    assert!(
        !shared
            .bib
            .search_articles("paragraphs", 5, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn downloaded_pdf_becomes_a_new_preferred_attachment_preserving_old_text_and_notes() {
    let storage = Arc::new(OpendalFileStorage::new_temp());
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(storage.clone());
    shared
        .bib
        .upsert_article(&Article::new("paper", "Paper"))
        .await
        .unwrap();
    shared
        .bib
        .upsert_fulltext(&legacy("paper", "Saved old text"))
        .await
        .unwrap();
    let old = shared.bib.get_fulltext("paper").await.unwrap().unwrap();
    let old = materialize_legacy(&shared, old).await.unwrap();
    let old_key = old.attachment_key.clone().unwrap();
    let note = shared
        .bib
        .add_annotation(
            "paper",
            AnnotationKind::Note,
            "Old text note",
            None,
            None,
            None,
            Some(&old_key),
        )
        .await
        .unwrap();
    let original = pdf();
    let expected = original.bytes.clone();
    let ft = store_pdf(&shared, "paper", original).await.unwrap();
    assert_ne!(ft.attachment_key, old.attachment_key);
    assert_eq!(ft.file_format, FileFormat::Pdf);
    assert_eq!(ft.extract_status, Some(ExtractStatus::Pending));
    assert_eq!(ft.file_size, Some(expected.len() as i64));
    assert_eq!(ft.file_hash.as_ref().unwrap().len(), 64);
    assert_eq!(
        shared
            .bib
            .get_fulltext("paper")
            .await
            .unwrap()
            .unwrap()
            .file_path,
        ft.file_path
    );
    assert_eq!(
        shared.bib.zotero_attachments("paper").await.unwrap().len(),
        2
    );
    assert_eq!(
        shared
            .bib
            .get_attachment_fulltext("paper", &old_key)
            .await
            .unwrap()
            .unwrap()
            .text_content
            .as_deref(),
        Some("Saved old text")
    );
    assert_eq!(
        shared
            .bib
            .list_annotations("paper", Some(&old_key))
            .await
            .unwrap()[0]
            .id,
        note.id
    );
    let bytes = bib_base::read_full(
        &storage,
        &bib_base::vfs_virtual_path(&ft.file_path).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(bytes.as_slice(), expected.as_slice());
}

#[tokio::test]
async fn auto_download_does_not_replace_user_selected_file_and_write_failures_leave_no_attachment()
{
    let storage = Arc::new(OpendalFileStorage::new_temp());
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(storage.clone());
    shared
        .bib
        .upsert_article(&Article::new("paper", "Paper"))
        .await
        .unwrap();
    let mut existing = legacy("paper", "User text");
    existing.source = FullTextSource::UserUpload;
    existing.file_path = "vfs:///literature/user.txt".into();
    existing.extracted_by = Some("simple".into());
    shared.bib.upsert_fulltext(&existing).await.unwrap();
    let key = shared
        .bib
        .zotero_attachment("paper")
        .await
        .unwrap()
        .unwrap()
        .key;
    store_pdf(&shared, "paper", pdf()).await.unwrap();
    assert_eq!(
        shared
            .bib
            .zotero_attachment("paper")
            .await
            .unwrap()
            .unwrap()
            .key,
        key
    );
    assert_eq!(
        shared
            .bib
            .get_fulltext("paper")
            .await
            .unwrap()
            .unwrap()
            .source,
        FullTextSource::UserUpload
    );
    let blocked = Arc::new(OpendalFileStorage::new_temp());
    blocked
        .write_bytes("/literature", b"blocked".to_vec())
        .await
        .unwrap();
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(blocked);
    shared
        .bib
        .upsert_article(&Article::new("paper", "Paper"))
        .await
        .unwrap();
    assert!(store_pdf(&shared, "paper", pdf()).await.is_err());
    assert!(
        shared
            .bib
            .zotero_attachments("paper")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(shared.bib.get_fulltext("paper").await.unwrap().is_none());
}
