//! End-to-end tool tests: drive the full deck tool surface against an
//! in-memory store (writing-base tools_e2e pattern).

use agentik_core::tools::ToolRegistration;
use ppt_base::shared::DeckShared;
use ppt_base::tools::deck_all_registrations;
use serde_json::{Value, json};

/// Execute a tool by name with the given JSON input.
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

/// Execute a tool expecting a failure (returns the error).
async fn call_tool_err(regs: &[ToolRegistration], name: &str, input: Value) -> String {
    let reg = regs
        .iter()
        .find(|r| r.definition.name == name)
        .unwrap_or_else(|| panic!("tool '{name}' not found"));
    let err = reg
        .implementation
        .execute(input)
        .await
        .expect_err("tool must fail");
    err.to_string()
}

#[tokio::test]
async fn full_deck_lifecycle_through_tools() {
    let shared = DeckShared::open_in_memory().await.unwrap();
    let regs = deck_all_registrations(shared.clone());

    // 5 deck + 4 slide + 1 asset + 2 export (export + status)
    assert_eq!(regs.len(), 12);

    // 1. deck_create (meeting template)
    let created = call_tool(
        &regs,
        "deck_create",
        json!({"title": "组会:文献报告", "template": "meeting"}),
    )
    .await;
    let deck_id = created["deck_id"].as_str().unwrap().to_string();
    assert!(created["slide_count"].as_u64().unwrap() >= 5);

    // 2. deck_outline
    let outline = call_tool(&regs, "deck_outline", json!({"deck_id": deck_id})).await;
    assert!(outline["slide_count"].as_u64().unwrap() >= 5);

    // 3. slide_patch — slide 2 content + note
    let patched = call_tool(
        &regs,
        "slide_patch",
        json!({
            "deck_id": deck_id,
            "no": 2,
            "content": "# 背景 Background\n\n- 更新后的要点",
            "note": "备注内容"
        }),
    )
    .await;
    assert!(patched["revision"].as_str().is_some());

    // 4. deck_get — patch landed; asset list present
    let got = call_tool(&regs, "deck_get", json!({"deck_id": deck_id})).await;
    let md = got["markdown"].as_str().unwrap();
    assert!(md.contains("更新后的要点"));
    assert!(md.contains("备注内容"));
    assert!(got["slides"].as_array().unwrap().len() >= 5);

    // 5. slide_insert — new slide after 2
    let ins = call_tool(
        &regs,
        "slide_insert",
        json!({
            "deck_id": deck_id,
            "after": 2,
            "content": "## 新插入页",
            "frontmatter_raw": "layout: section"
        }),
    )
    .await;
    assert_eq!(ins["inserted_no"].as_u64(), Some(3));
    let count_after_insert = ins["slide_count"].as_u64().unwrap();

    // 6. slide_move — to the end
    let moved = call_tool(
        &regs,
        "slide_move",
        json!({"deck_id": deck_id, "from": 3, "to": count_after_insert}),
    )
    .await;
    assert_eq!(
        moved["slide_count"].as_u64().unwrap(),
        count_after_insert
    );

    // 7. slide_remove
    let removed = call_tool(
        &regs,
        "slide_remove",
        json!({"deck_id": deck_id, "no": count_after_insert}),
    )
    .await;
    assert_eq!(
        removed["slide_count"].as_u64().unwrap(),
        count_after_insert - 1
    );

    // 8. asset_attach — 1x1 PNG
    let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let asset = call_tool(
        &regs,
        "asset_attach",
        json!({
            "deck_id": deck_id,
            "filename": "fig1.png",
            "data_base64": png_b64,
            "caption": "Figure 1",
            "provenance": "dag://run/1/plot"
        }),
    )
    .await;
    let asset_id = asset["asset_id"].as_str().unwrap().to_string();
    assert!(asset["ref"].as_str().unwrap().starts_with("assets://"));

    let meta = shared.store.get_asset(&deck_id, &asset_id).await.unwrap();
    let blob = shared.assets.get(&meta.blob_key).await.unwrap();
    assert!(!blob.is_empty());
    assert_eq!(meta.provenance.as_deref(), Some("dag://run/1/plot"));

    // 9. deck_export — queued row (no worker in this test process; the
    // row stays queued and status is observable)
    let exported = call_tool(
        &regs,
        "deck_export",
        json!({"deck_id": deck_id, "format": "pdf"}),
    )
    .await;
    assert_eq!(exported["status"], "queued");
    let export_id = exported["export_id"].as_str().unwrap().to_string();
    let status = call_tool(
        &regs,
        "deck_export_status",
        json!({"export_id": export_id}),
    )
    .await;
    assert_eq!(status["status"], "queued");
    assert_eq!(status["deck_id"], deck_id.as_str());

    // 10. deck_list
    let listed = call_tool(&regs, "deck_list", json!({"filter": "文献报告"})).await;
    assert_eq!(listed["total"].as_u64(), Some(1));

    // 11. deck_delete
    call_tool(&regs, "deck_delete", json!({"deck_id": deck_id})).await;
    let listed = call_tool(&regs, "deck_list", json!({})).await;
    assert_eq!(listed["total"].as_u64(), Some(0));
}

#[tokio::test]
async fn headmatter_guard_surfaces_through_tool() {
    let shared = DeckShared::open_in_memory().await.unwrap();
    let regs = deck_all_registrations(shared);

    let created = call_tool(&regs, "deck_create", json!({"title": "T"})).await;
    let deck_id = created["deck_id"].as_str().unwrap();

    let err = call_tool_err(
        &regs,
        "slide_patch",
        json!({
            "deck_id": deck_id,
            "no": 1,
            "frontmatter_raw": "layout: cover"
        }),
    )
    .await;
    assert!(err.contains("headmatter"), "error mentions headmatter: {err}");
}

#[tokio::test]
async fn bad_yaml_rejected_through_tool() {
    let shared = DeckShared::open_in_memory().await.unwrap();
    let regs = deck_all_registrations(shared);

    let created = call_tool(&regs, "deck_create", json!({"title": "T"})).await;
    let deck_id = created["deck_id"].as_str().unwrap();

    // grow the deck first so slide 2 exists
    call_tool(
        &regs,
        "slide_insert",
        json!({"deck_id": deck_id, "after": 1, "content": "## s2"}),
    )
    .await;

    let err = call_tool_err(
        &regs,
        "slide_patch",
        json!({
            "deck_id": deck_id,
            "no": 2,
            "content": "fine content",
            "frontmatter_raw": "layout: [unclosed"
        }),
    )
    .await;
    assert!(err.contains("YAML") || err.contains("yaml"), "{err}");
}

#[tokio::test]
async fn slide_out_of_range_error_mentions_total() {
    let shared = DeckShared::open_in_memory().await.unwrap();
    let regs = deck_all_registrations(shared);

    let created = call_tool(&regs, "deck_create", json!({"title": "T"})).await;
    let deck_id = created["deck_id"].as_str().unwrap();

    let err = call_tool_err(
        &regs,
        "slide_patch",
        json!({"deck_id": deck_id, "no": 99, "content": "x"}),
    )
    .await;
    assert!(err.contains("99"), "{err}");
    assert!(err.contains("1-"), "error mentions valid range: {err}");
}
