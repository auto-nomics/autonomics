//! Citation e2e: `deck_references` against a real in-memory BibBase —
//! keys resolve, the References slide lands, reruns refresh in place.

use agentik_core::tools::ToolRegistration;
use bib_base::BibBase;
use bib_types::Article;
use ppt_base::DeckHandle;
use ppt_base::shared::DeckShared;
use ppt_base::tools::deck_all_registrations;
use serde_json::{json, Value};
use std::sync::Arc;

async fn call_tool(regs: &[ToolRegistration], name: &str, input: Value) -> Value {
    let reg = regs
        .iter()
        .find(|r| r.definition.name == name)
        .unwrap_or_else(|| panic!("tool '{name}' not found"));
    let result = reg
        .implementation
        .execute(input)
        .await
        .unwrap_or_else(|e| panic!("tool '{name}' failed: {e}"));
    match &result.content {
        agentik_sdk::types::ToolResultContent::Json(val) => val.clone(),
        agentik_sdk::types::ToolResultContent::Text(text) => {
            serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text }))
        }
        _ => json!({}),
    }
}

fn article(id: &str, doi: &str, title: &str, year: u16) -> Article {
    Article {
        id: id.into(),
        title: title.into(),
        authors: vec![bib_types::Author {
            last_name: "Smith".into(),
            fore_name: Some("Jane".into()),
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        }],
        identifiers: vec![bib_types::Identifier::new(bib_types::IdKind::Doi, doi)],
        year: Some(year),
        journal: Some("Nature Genetics".into()),
        source: Default::default(),
        ..minimal_rest()
    }
}

#[allow(deprecated)]
fn minimal_rest() -> Article {
    // Article has no Default; fill the remaining fields explicitly.
    serde_json::from_value(json!({
        "id": "", "title": "", "identifiers": []
    }))
    .unwrap()
}

#[tokio::test]
async fn deck_references_generates_and_refreshes() {
    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
    bib.upsert_article(&article(
        "a1",
        "10.1038/s41588-024-0001",
        "Fine-mapping bold loci",
        2024,
    ))
    .await
    .unwrap();
    bib.upsert_article(&article(
        "a2",
        "10.1038/s41588-023-0002",
        "Earlier companion paper",
        2023,
    ))
    .await
    .unwrap();

    let shared = std::sync::Arc::new(
        DeckShared::open_in_memory()
            .await
            .unwrap()
            .with_bib(Some(bib.clone())),
    );
    let handle = DeckHandle::spawn(shared.clone());
    let regs = deck_all_registrations(handle);

    // deck with two cite markers (meeting template's references slide is a
    // plain slide — no layout: references — so deck_references appends).
    let created = call_tool(
        &regs,
        "deck_create",
        json!({"title": "组会", "template": "meeting"}),
    )
    .await;
    let deck_id = created["deck_id"].as_str().unwrap().to_string();

    // patch slide 2 to carry cite markers
    call_tool(
        &regs,
        "slide_patch",
        json!({
            "deck_id": deck_id,
            "no": 2,
            "content": "## 背景\n\n关键结论 {{cite:smith2024finemapping}} 与 {{cite:smith2023earlier}}"
        }),
    )
    .await;

    let out = call_tool(&regs, "deck_references", json!({"deck_id": deck_id})).await;
    assert_eq!(out["resolved"], json!(2), "{out}");
    assert_eq!(out["unresolved"], json!([]));

    // the last slide is now the references slide with formatted entries
    let got = call_tool(&regs, "deck_get", json!({"deck_id": deck_id})).await;
    let md = got["markdown"].as_str().unwrap();
    assert!(md.contains("## 参考文献"));
    assert!(md.contains("J. Smith."));
    assert!(md.contains("doi:10.1038/s41588-024-0001"));
    assert!(md.contains("layout: references"));

    // rerun: refreshes the existing references slide (no duplicate)
    let before_count = got["slides"].as_array().unwrap().len();
    let out2 = call_tool(&regs, "deck_references", json!({"deck_id": deck_id})).await;
    assert_eq!(out2["resolved"], json!(2));
    let got2 = call_tool(&regs, "deck_get", json!({"deck_id": deck_id})).await;
    assert_eq!(got2["slides"].as_array().unwrap().len(), before_count);
    let occurrences = got2["markdown"]
        .as_str()
        .unwrap()
        .matches("## 参考文献")
        .count();
    assert_eq!(occurrences, 1, "references slide replaced, not duplicated");

    // unresolved keys are reported and excluded
    call_tool(
        &regs,
        "slide_patch",
        json!({"deck_id": deck_id, "no": 2, "content": "新增 {{cite:ghost2020}}"}),
    )
    .await;
    let out3 = call_tool(&regs, "deck_references", json!({"deck_id": deck_id})).await;
    assert_eq!(out3["unresolved"], json!(["ghost2020"]));
}

#[tokio::test]
async fn deck_references_without_bib_reports_clearly() {
    let handle = DeckHandle::spawn_in_memory().await.unwrap();
    let regs = deck_all_registrations(handle);
    let created = call_tool(&regs, "deck_create", json!({"title": "T"})).await;
    let deck_id = created["deck_id"].as_str().unwrap();

    let reg = regs
        .iter()
        .find(|r| r.definition.name == "deck_references")
        .unwrap();
    let err = reg
        .implementation
        .execute(json!({"deck_id": deck_id}))
        .await
        .expect_err("must fail without bib");
    assert!(err.to_string().contains("bibliography"), "{err}");
}
