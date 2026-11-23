//! Tool-layer citation-key tests: tools accept citekeys wherever article
//! references are taken, and surface the assigned key in outputs.

use agentik_core::tools::ToolFunction;
use agentik_sdk::types::ToolResultContent;
use bib_base::BibBase;
use bib_base::library_tools::{BibGetArticleInput, BibGetArticleTool};
use bib_types::{Article, Author};

async fn seeded_db() -> (BibBase, String) {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut art = Article::new("ref-paper-1", "Citekey resolution paper");
    art.authors.push(Author {
        last_name: "Garcia".into(),
        fore_name: Some("M".into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    art.year = Some(2023);
    db.upsert_article(&art).await.unwrap();
    let key = db
        .get_article("ref-paper-1")
        .await
        .unwrap()
        .unwrap()
        .citekey
        .expect("assigned");
    (db, key)
}

#[tokio::test]
async fn bib_get_article_resolves_citekey_and_echoes_it() {
    let (db, key) = seeded_db().await;
    let tool = BibGetArticleTool { bib: std::sync::Arc::new(db) };

    let out = tool
        .run(BibGetArticleInput {
            article_ids: vec![key.clone()],
        })
        .await
        .unwrap();

    let payload = match out.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected json result, got {other:?}"),
    };
    let results = payload["results"].as_array().unwrap();
    assert_eq!(results[0]["found"], serde_json::json!(true));
    assert_eq!(results[0]["article_id"], serde_json::json!("ref-paper-1"));
    assert_eq!(results[0]["citekey"], serde_json::json!(key));
}

#[tokio::test]
async fn bib_get_article_reports_misses_for_unknown_citekeys() {
    let (db, _) = seeded_db().await;
    let tool = BibGetArticleTool { bib: std::sync::Arc::new(db) };

    let out = tool
        .run(BibGetArticleInput {
            article_ids: vec!["nobody1999missing-zzzz".into()],
        })
        .await
        .unwrap();

    let payload = match out.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected json result, got {other:?}"),
    };
    let results = payload["results"].as_array().unwrap();
    assert_eq!(results[0]["found"], serde_json::json!(false));
}

#[tokio::test]
async fn bib_resolve_citekeys_reports_metadata_and_misses() {
    use agentik_sdk::types::ToolResultContent;
    use bib_base::library_tools::{BibResolveCitekeysInput, BibResolveCitekeysTool};

    let (db, key) = seeded_db().await;
    let tool = BibResolveCitekeysTool {
        bib: std::sync::Arc::new(db),
    };
    let out = tool
        .run(BibResolveCitekeysInput {
            citekeys: vec![key.clone(), "ghost2000missing".into()],
        })
        .await
        .unwrap();
    let payload = match out.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected json, got {other:?}"),
    };
    assert_eq!(payload["found"], serde_json::json!(1));
    assert_eq!(payload["missing"], serde_json::json!(1));
    let results = payload["results"].as_array().unwrap();
    assert_eq!(results[0]["found"], serde_json::json!(true));
    assert_eq!(results[0]["title"], serde_json::json!("Citekey resolution paper"));
    assert_eq!(results[0]["citekey"], serde_json::json!(key));
    assert_eq!(results[1]["found"], serde_json::json!(false));
}

#[tokio::test]
async fn literature_import_manual_dedupes_and_assigns_citekey() {
    use agentik_sdk::types::ToolResultContent;
    use bib_base::library_tools::{LiteratureImportManualInput, LiteratureImportManualTool};

    let mut shared = bib_base::BibShared::open_in_memory().await.unwrap();
    shared.file_storage = Some(std::sync::Arc::new(vfs::OpendalFileStorage::new_temp()));
    let tool = LiteratureImportManualTool {
        shared: std::sync::Arc::new(shared),
        page: None,
    };

    let json = |out: agentik_sdk::types::ToolResult| match out.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected json, got {other:?}"),
    };

    // 新文章:导入并分配 citekey
    let first = json(
        tool.run(LiteratureImportManualInput {
            title: "Browser-discovered paper".into(),
            authors: Some(vec!["Chen Wei".into(), "Park Soo".into()]),
            year: Some(2025),
            journal: Some("Nature Methods".into()),
            doi: Some("10.9999/browser-find".into()),
            pmid: None,
            arxiv: None,
            url: Some("https://example.org/paper".into()),
            abstract_text: Some("Found on a journal page.".into()),
            archive_current_page: None,
        })
        .await
        .unwrap(),
    );
    assert_eq!(first["status"], serde_json::json!("imported"));
    let key = first["citekey"].as_str().unwrap().to_string();
    assert!(key.starts_with("chen2025browserdiscovered-"), "{first}");

    // 同 DOI 再导:already_present,不覆盖
    let second = json(
        tool.run(LiteratureImportManualInput {
            title: "Renamed later version".into(),
            authors: None,
            year: None,
            journal: None,
            doi: Some("10.9999/browser-find".into()),
            pmid: None,
            arxiv: None,
            url: None,
            abstract_text: None,
            archive_current_page: None,
        })
        .await
        .unwrap(),
    );
    assert_eq!(second["status"], serde_json::json!("already_present"));
    assert_eq!(second["citekey"].as_str(), Some(key.as_str()));
    assert_eq!(second["title"], serde_json::json!("Browser-discovered paper"));

    // 空标题拒绝
    let err = tool
        .run(LiteratureImportManualInput {
            title: "   ".into(),
            authors: None,
            year: None,
            journal: None,
            doi: None,
            pmid: None,
            arxiv: None,
            url: None,
            abstract_text: None,
            archive_current_page: None,
        })
        .await;
    assert!(err.is_err());
}

/// Fake live page for the archive path — no browser needed.
struct FakePage;

#[async_trait::async_trait]
impl bib_base::library_tools::CurrentPageSource for FakePage {
    async fn current_page(&self) -> Result<(String, String, String), String> {
        Ok((
            "https://example.org/paper".into(),
            "Example Paper Page".into(),
            "<html><body>full page content</body></html>".into(),
        ))
    }
}

#[tokio::test]
async fn manual_import_archives_current_page_html_as_attachment() {
    use agentik_sdk::types::ToolResultContent;
    use bib_base::library_tools::LiteratureImportManualInput;

    let mut shared = bib_base::BibShared::open_in_memory().await.unwrap();
    shared.file_storage = Some(std::sync::Arc::new(vfs::OpendalFileStorage::new_temp()));
    let tool = bib_base::library_tools::LiteratureImportManualTool {
        shared: std::sync::Arc::new(shared.clone()),
        page: Some(std::sync::Arc::new(FakePage)),
    };
    let json = |out: agentik_sdk::types::ToolResult| match out.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected json, got {other:?}"),
    };

    let out = json(
        tool.run(LiteratureImportManualInput {
            title: "Archived find".into(),
            authors: Some(vec!["Doe Jane".into()]),
            year: Some(2026),
            journal: None,
            doi: Some("10.9999/archived".into()),
            pmid: None,
            arxiv: None,
            url: None,
            abstract_text: None,
            archive_current_page: Some(true),
        })
        .await
        .unwrap(),
    );
    assert_eq!(out["status"], serde_json::json!("imported"));
    let attachment = &out["attachment"];
    let key = attachment["attachment_key"].as_str().unwrap();
    assert!(!key.is_empty());
    assert!(attachment["filename"].as_str().unwrap().ends_with(".html"));
    assert_eq!(attachment["source_url"], serde_json::json!("https://example.org/paper"));
    assert_eq!(attachment["extraction_queued"], serde_json::json!(true));

    // The attachment is real: fulltexts row + readable bytes.
    let article_id = out["article_id"].as_str().unwrap();
    let got = shared
        .bib
        .get_article(article_id)
        .await
        .unwrap()
        .expect("article");
    let _ = got;
    let ft = shared.bib.get_fulltext(article_id).await.unwrap().expect("fulltext row");
    assert_eq!(ft.file_format.as_str(), "html");
    assert_eq!(ft.attachment_key.as_deref(), Some(key));
    // extraction races from pending to running/done in the background —
    // any of the three proves the queue was entered
    assert!(matches!(
        ft.extract_status.map(|s| s.as_str()),
        Some("pending") | Some("running") | Some("done")
    ));
}
