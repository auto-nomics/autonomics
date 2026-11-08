//! Diagnostics system end-to-end: stub render backend + worker trigger +
//! deck_diagnose tool + report freshness.

use std::sync::Arc;
use std::time::Duration;

use agentik_core::tools::ToolRegistration;
use ppt_base::export::{DeckExporter, ExportJob, ExportOutput};
use ppt_base::shared::DeckShared;
use ppt_base::tools::deck_all_registrations;
use ppt_base::DeckHandle;
use serde_json::{Value, json};

/// Render backend stub: succeeds with canned per-slide findings.
#[derive(Default)]
struct StubBackend;

#[async_trait::async_trait]
impl DeckExporter for StubBackend {
    fn name(&self) -> &'static str {
        "stub-diagnose"
    }

    async fn export(&self, _job: ExportJob) -> ppt_base::Result<ExportOutput> {
        Err(ppt_base::Error::Export("not needed in this test".into()))
    }

    async fn diagnose_slides(&self, job: &ExportJob, targets: &[(u32, u32)]) -> ppt_base::Result<Value> {
        let mut map = serde_json::Map::new();
        for (no, _) in targets {
            map.insert(
                no.to_string(),
                json!([
                    { "rule": "text-overflow-canvas", "severity": "error",
                      "message": format!("slide {no} text overflows the right edge by 24px"),
                      "details": { "overflow": "right edge by 24px" } }
                ]),
            );
        }
        Ok(Value::Object(map))
    }
}

async fn stub_shared() -> Arc<DeckShared> {
    let assets = ppt_base::BlobArea::in_memory("deck-assets");
    let exports = ppt_base::BlobArea::in_memory("deck-exports");
    Arc::new(
        DeckShared::open_with(":memory:", assets, exports, Arc::new(StubBackend))
            .await
            .unwrap(),
    )
}

/// Wait until the stored report is fresh for the deck's current state
/// (worker runs are asynchronous; give them a generous deadline).
async fn await_fresh(shared: &DeckShared, deck_id: &str) -> ppt_base::store::DiagnosticsRow {
    for _ in 0..200 {
        let deck = shared.load_deck(deck_id).await.unwrap();
        let theme = shared.theme_css(deck_id).await.unwrap();
        let theme_rev = ppt_base::shared::css_revision(&theme);
        if let Ok(Some(report)) = shared.latest_diagnostics(deck_id).await {
            if report.is_fresh(&deck.meta.revision, &theme_rev) {
                return report;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("diagnostics report never became fresh for {deck_id}");
}

#[tokio::test]
async fn write_triggers_worker_and_merges_tiers() {
    let shared = stub_shared().await;
    shared.start_diagnostics_worker();
    let handle = DeckHandle::spawn(shared.clone());
    let deck = shared.create_deck("诊断测试", Some("meeting")).await.unwrap();
    let id = deck.meta.id.clone();

    // Several rapid writes — the worker must collapse them and still
    // end at a fresh report for the LAST state.
    for i in 0..3 {
        let markdown = format!("# v{i} 标题\n\n---\n\n## 第二页\n\n- 要点\n");
        let deck_id = id.clone();
        handle
            .write(move |s| {
                Box::pin(async move { s.save_markdown(&deck_id, &markdown, "edit").await.map(|_| ()) })
            })
            .await
            .unwrap();
    }

    let report = await_fresh(&shared, &deck.meta.id).await;
    assert!(report.rendered, "stub backend should have run the render tier");
    // Render finding present (slide 1 of 2 flagged by the stub).
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "text-overflow-canvas" && f.severity == ppt_base::diagnostics::Severity::Error),
        "{:?}",
        report.findings
    );
}

#[tokio::test]
async fn theme_css_write_also_triggers() {
    let shared = stub_shared().await;
    shared.start_diagnostics_worker();
    let deck = shared.create_deck("主题触发", None).await.unwrap();
    let id = deck.meta.id.clone();

    let current = shared.theme_css(&id).await.unwrap();
    shared
        .save_theme_css(&id, ":root { --deck-width: 1280px; --deck-height: 720px; }", &ppt_base::shared::css_revision(&current))
        .await
        .unwrap();

    let report = await_fresh(&shared, &id).await;
    assert!(report.rendered);
}

#[tokio::test]
async fn deck_diagnose_tool_serves_fresh_then_reruns() {
    let shared = stub_shared().await;
    let handle = DeckHandle::spawn(shared.clone());
    let regs: Vec<ToolRegistration> = deck_all_registrations(handle.clone());
    let deck = shared.create_deck("工具面", None).await.unwrap();
    let id = deck.meta.id.clone();

    let find = |regs: &[ToolRegistration]| {
        regs.iter()
            .find(|r| r.definition.name == "deck_diagnose")
            .unwrap()
            .clone()
    };
    let tool = find(&regs);
    let result = tool
        .implementation
        .execute(json!({ "deck_id": id }))
        .await
        .unwrap();
    let payload = match &result.content {
        agentik_sdk::types::ToolResultContent::Json(v) => v.clone(),
        agentik_sdk::types::ToolResultContent::Text(t) => serde_json::from_str(t).unwrap(),
        _ => panic!("unexpected content"),
    };
    // No worker armed: the tool ran the synchronous path (stub backend).
    assert_eq!(payload["rendered"], json!(true));
    assert_eq!(payload["errors"].as_u64().unwrap(), payload["findings"].as_array().unwrap().len() as u64);
}

#[tokio::test]
async fn null_backend_degrades_to_static_tier() {
    // In-memory shared (NullExporter): render tier unavailable → static-only.
    let shared = DeckShared::open_in_memory().await.unwrap();
    let report = ppt_base::diagnostics::run_diagnostics(&shared, "does-not-exist")
        .await;
    assert!(report.is_err(), "unknown deck must error, not fabricate a report");

    let deck = shared.create_deck("静态", None).await.unwrap();
    let markdown = "# 标题\n\n---\n\n\n\n---\n\n# 三\n";
    shared.save_markdown(&deck.meta.id, &markdown, "x").await.unwrap();
    let report = ppt_base::diagnostics::run_diagnostics(&shared, &deck.meta.id).await.unwrap();
    assert!(!report.rendered);
    assert!(report.findings.iter().any(|f| f.rule == "empty-slide" && f.slide == 2));
    // Report is stored and fresh.
    let stored = shared.latest_diagnostics(&deck.meta.id).await.unwrap().unwrap();
    assert!(stored.is_fresh(&report.revision, &report.theme_revision));
}

#[tokio::test]
async fn write_results_report_scheduled_render_tier_when_worker_armed() {
    let shared = stub_shared().await;
    shared.start_diagnostics_worker();
    let handle = DeckHandle::spawn(shared.clone());
    let regs: Vec<ToolRegistration> = deck_all_registrations(handle.clone());
    let deck = shared.create_deck("scheduled", Some("meeting")).await.unwrap();
    let id = deck.meta.id.clone();

    let tool = regs.iter().find(|r| r.definition.name == "slide_patch").unwrap().clone();
    let result = tool
        .implementation
        .execute(json!({ "deck_id": id, "no": 2, "content": "# Updated\n\n- point" }))
        .await
        .unwrap();
    let payload = match &result.content {
        agentik_sdk::types::ToolResultContent::Json(v) => v.clone(),
        agentik_sdk::types::ToolResultContent::Text(t) => serde_json::from_str(t).unwrap(),
        _ => panic!("unexpected content"),
    };
    assert_eq!(payload["diagnostics"]["renderTier"], json!("scheduled"), "{payload}");
    assert!(payload["diagnostics"]["note"].as_str().unwrap().contains("deck_diagnose"));
}
