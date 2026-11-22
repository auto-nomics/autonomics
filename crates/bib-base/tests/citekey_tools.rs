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
