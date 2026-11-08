//! deck_slide_preview e2e: a stub exporter returns a fixed PNG; the tool
//! must return an image block with slide metadata, and the ephemeral
//! print-token row must be deleted afterwards (token dies with it).

use agentik_core::tools::ToolRegistration;
use ppt_base::DeckHandle;
use ppt_base::export::{DeckExporter, ExportJob, ExportOutput};
use ppt_base::shared::DeckShared;
use ppt_base::tools::deck_all_registrations;
use base64::Engine as _;
use serde_json::json;
use std::sync::Arc;

/// 1×1 PNG stub exporter.
struct StubPreviewExporter;

#[async_trait::async_trait]
impl DeckExporter for StubPreviewExporter {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn export(&self, _job: ExportJob) -> ppt_base::Result<ExportOutput> {
        Err(ppt_base::Error::Export("not used".into()))
    }

    async fn preview_png(
        &self,
        _job: &ExportJob,
        slide_no: u32,
        clicks: u32,
    ) -> ppt_base::Result<Vec<u8>> {
        // echo the requested state so the test can verify threading
        Ok(format!("PNG slide={slide_no} clicks={clicks}").into_bytes())
    }
}

async fn call_tool(regs: &[ToolRegistration], name: &str, input: serde_json::Value) -> agentik_sdk::types::ToolResult {
    let reg = regs
        .iter()
        .find(|r| r.definition.name == name)
        .unwrap_or_else(|| panic!("tool '{name}' not found"));
    reg.implementation
        .execute(input)
        .await
        .unwrap_or_else(|e| panic!("tool '{name}' failed: {e}"))
}

#[tokio::test]
async fn preview_returns_image_block_and_cleans_up() {
    let shared = Arc::new(
        DeckShared::open_in_memory()
            .await
            .unwrap()
            .with_exporter(Arc::new(StubPreviewExporter)),
    );
    let handle = DeckHandle::spawn(shared.clone());
    let regs = deck_all_registrations(handle.clone());

    let created = call_tool(&regs, "deck_create", json!({"title": "预览", "template": "defense"})).await;
    // deck_create returns Json content — extract id
    let deck_id = match &created.content {
        agentik_sdk::types::ToolResultContent::Json(v) => {
            v["deck_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected json, got {other:?}"),
    };

    // meeting template slide with clicks: find one with a click plan
    let outline = call_tool(&regs, "deck_outline", json!({"deck_id": deck_id.clone()})).await;
    let slides = match &outline.content {
        agentik_sdk::types::ToolResultContent::Json(v) => v["outline"].as_array().unwrap().clone(),
        other => panic!("expected json, got {other:?}"),
    };
    let click_slide = slides
        .iter()
        .find(|s| s["clicksTotal"].as_u64().unwrap_or(0) > 0)
        .expect("defense template has a click slide");
    let no = click_slide["no"].as_u64().unwrap();
    let total = click_slide["clicksTotal"].as_u64().unwrap();

    // fully-revealed preview (default clicks)
    let result = call_tool(
        &regs,
        "deck_slide_preview",
        json!({"deck_id": deck_id, "no": no}),
    )
    .await;
    let blocks = match result.content {
        agentik_sdk::types::ToolResultContent::Blocks(b) => b,
        other => panic!("expected blocks, got {other:?}"),
    };
    assert_eq!(blocks.len(), 2);
    match &blocks[0] {
        agentik_sdk::types::ToolResultBlock::Text { text } => {
            assert!(text.contains(&format!("slide {}/", no)), "{text}");
            assert!(text.contains(&format!("clicks {}/{}", total, total)), "{text}");
        }
        other => panic!("expected text block, got {other:?}"),
    }
    match &blocks[1] {
        agentik_sdk::types::ToolResultBlock::Image { source } => {
            let agentik_sdk::types::ToolImageSource::Base64 { media_type, data } = source;
            assert_eq!(media_type, "image/png");
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            let text = String::from_utf8(decoded).unwrap();
            assert!(text.contains(&format!("slide={}", no)), "{text}");
            assert!(text.contains(&format!("clicks={}", total)), "{text}");
        }
        other => panic!("expected image block, got {other:?}"),
    }

    // clicks=0 threads through
    let result = call_tool(
        &regs,
        "deck_slide_preview",
        json!({"deck_id": deck_id, "no": no, "clicks": 0}),
    )
    .await;
    match result.content {
        agentik_sdk::types::ToolResultContent::Blocks(b) => match &b[1] {
            agentik_sdk::types::ToolResultBlock::Image { source } => {
                let agentik_sdk::types::ToolImageSource::Base64 { data, .. } = source;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                let text = String::from_utf8(decoded).unwrap();
                assert!(text.contains("clicks=0"), "{text}");
            }
            other => panic!("expected image block, got {other:?}"),
        },
        other => panic!("expected blocks, got {other:?}"),
    }

    // ephemeral token row is gone (preview rows never linger)
    let exports = shared.store.list_exports(&deck_id).await.unwrap();
    assert!(
        exports.iter().all(|e| !e.id.starts_with("preview-")),
        "preview rows must be deleted, found: {:?}",
        exports.iter().map(|e| e.id.as_str()).collect::<Vec<_>>()
    );

    // out-of-range slide → error mentioning the deck total
    let err = regs
        .iter()
        .find(|r| r.definition.name == "deck_slide_preview")
        .unwrap()
        .implementation
        .execute(json!({"deck_id": deck_id, "no": 99}))
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("(1-"), "{err}");
}

#[tokio::test]
async fn preview_without_render_backend_errors_clearly() {
    // NullExporter: actionable error, no hang
    let handle = DeckHandle::spawn_in_memory().await.unwrap();
    let regs = deck_all_registrations(handle.clone());
    let created = call_tool(&regs, "deck_create", json!({"title": "T"})).await;
    let deck_id = match &created.content {
        agentik_sdk::types::ToolResultContent::Json(v) => {
            v["deck_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected json, got {other:?}"),
    };
    let err = regs
        .iter()
        .find(|r| r.definition.name == "deck_slide_preview")
        .unwrap()
        .implementation
        .execute(json!({"deck_id": deck_id, "no": 1}))
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("preview"), "{err}");
}
