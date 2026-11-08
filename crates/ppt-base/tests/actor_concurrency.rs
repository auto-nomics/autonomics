//! Actor concurrency: concurrent slide patches through the write worker
//! must serialize — every patch lands, none is lost to a read-modify-write
//! race (the failure mode of the direct-handle shape).

use agentik_core::tools::ToolRegistration;
use ppt_base::DeckHandle;
use ppt_base::tools::deck_all_registrations;
use serde_json::json;
use std::sync::Arc;

async fn call_tool(regs: &[ToolRegistration], name: &str, input: serde_json::Value) -> serde_json::Value {
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

#[tokio::test]
async fn concurrent_slide_inserts_all_land() {
    let handle = DeckHandle::spawn_in_memory().await.unwrap();
    let regs = Arc::new(deck_all_registrations(handle.clone()));

    let created = call_tool(&regs, "deck_create", json!({"title": "并发"})).await;
    let deck_id = created["deck_id"].as_str().unwrap().to_string();

    // 8 concurrent inserts of distinct slides after slide 1.
    let mut tasks = Vec::new();
    for i in 0..8 {
        let regs = regs.clone();
        let deck_id = deck_id.clone();
        tasks.push(tokio::spawn(async move {
            call_tool(
                &regs,
                "slide_insert",
                json!({
                    "deck_id": deck_id,
                    "after": 1,
                    "content": format!("## 并发页 {i}")
                }),
            )
            .await
        }));
    }
    for task in tasks {
        let out = task.await.unwrap();
        // every insert must succeed
        assert!(
            out["slide_count"].as_u64().is_some(),
            "insert failed: {out}"
        );
    }

    let got = call_tool(&regs, "deck_get", json!({"deck_id": deck_id, "include_markdown": true})).await;
    let md = got["markdown"].as_str().unwrap();
    // deck grew by exactly 8: no lost update, no duplicated version base
    for i in 0..8 {
        assert!(
            md.contains(&format!("并发页 {i}")),
            "slide {i} missing — a concurrent write was lost"
        );
    }
    let slide_count = got["slides"].as_array().unwrap().len();
    let initial = created["slide_count"].as_u64().unwrap() as usize;
    assert_eq!(slide_count, initial + 8, "expected {initial}+8 slides, got {slide_count}");
}

#[tokio::test]
async fn write_worker_sweeps_stale_exports_on_start() {
    // A store with a queued export row from a "previous daemon life";
    // spawning the actor must flip it to an actionable error.
    let shared = Arc::new(
        ppt_base::shared::DeckShared::open_in_memory()
            .await
            .unwrap(),
    );
    shared
        .store
        .create_deck("deck-1", "T", "# a\n", "rev", "")
        .await
        .unwrap();
    let record = ppt_base::store::ExportRecord {
        id: "exp-stale".into(),
        deck_id: "deck-1".into(),
        format: ppt_base::store::ExportFormat::Pdf,
        status: ppt_base::store::ExportStatus::Queued,
        params: "{}".into(),
        print_token_hash: Some("hashed".into()),
        token_expires_at: None,
        artifact_key: None,
        artifact_mime: None,
        artifact_size: None,
        error: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    shared.store.create_export(&record).await.unwrap();

    let _handle = DeckHandle::spawn(shared.clone());
    // The sweep runs before the worker's first tick; give it a beat.
    for _ in 0..50 {
        let row = shared.store.get_export("exp-stale").await.unwrap();
        if row.status == ppt_base::store::ExportStatus::Error {
            assert!(
                row.error.as_deref().unwrap_or_default().contains("restart"),
                "error should mention restart: {:?}",
                row.error
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("stale queued export was not swept to error");
}
