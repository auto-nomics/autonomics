//! Export tool: kick off PDF/PNG/PPTX generation.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{tool_err, DeckToolState};
use crate::shared::DeckShared;

/// Build export tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(DeckExportTool {
        shared: state.shared,
    })]
}

// ===========================================================================
// deck_export
// ===========================================================================

#[tool(
    name = "deck_export",
    description = "Export a deck to PDF (one page per slide, exact page size from the aspect \
                   ratio), PNG (one image per slide, DPI-aware for journal figure specs), or \
                   PPTX (slides as full-bleed images + real speaker notes, editable page size). \
                   Runs asynchronously; poll deck_export_status. \
                  \
                  **Examples**: \
                  • deck_id=\"deck-1\", format=\"pdf\" \
                  • deck_id=\"deck-1\", format=\"png\", dpi=300, range=\"1-5\""
)]
pub struct DeckExportInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "Output format: \"pdf\", \"png\", or \"pptx\""]
    pub format: String,
    #[desc = "PNG DPI: 96 (screen, default 192 = 2x), up to 300 (journal print)"]
    pub dpi: Option<u32>,
    #[desc = "Slide range, 1-based: \"1,4-5\" (optional; default all slides)"]
    pub range: Option<String>,
}

struct DeckExportTool {
    shared: DeckShared,
}

#[async_trait]
impl ToolFunction for DeckExportTool {
    type Input = DeckExportInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let format = crate::store::ExportFormat::parse(&input.format).ok_or_else(|| {
            agentik_core::tools::ToolError::ExecutionFailed {
                source: Box::new(crate::Error::Export(format!(
                    "unknown export format {:?} (pdf | png | pptx)",
                    input.format
                ))),
            }
        })?;

        let deck = self.shared.load_deck(&input.deck_id).await.map_err(tool_err)?;
        let pages = deck.resolve_range(input.range.as_deref()).map_err(|e| {
            agentik_core::tools::ToolError::ExecutionFailed {
                source: Box::new(crate::Error::BadRange(e)),
            }
        })?;

        let dpi = input.dpi.unwrap_or(192).clamp(96, 300);

        let export_id = format!("exp-{}", uuid::Uuid::new_v4().simple());
        let now = chrono::Utc::now().to_rfc3339();
        let record = crate::store::ExportRecord {
            id: export_id.clone(),
            deck_id: input.deck_id.clone(),
            format,
            status: crate::store::ExportStatus::Queued,
            params: serde_json::json!({
                "dpi": dpi,
                "pages": pages.iter().map(|p| p + 1).collect::<Vec<_>>(),
            })
            .to_string(),
            print_token_hash: None,
            token_expires_at: None,
            artifact_key: None,
            artifact_mime: None,
            artifact_size: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.shared.store.create_export(&record).await.map_err(tool_err)?;

        // Run the configured exporter inline (M8 adds the queue + Chromium
        // backend; NullExporter records an actionable error here).
        let job = crate::export::ExportJob {
            deck_id: input.deck_id.clone(),
            export_id: export_id.clone(),
            format,
            dpi,
            range: input.range.unwrap_or_default(),
            print_url: String::new(),
        };
        let mut rec = self
            .shared
            .store
            .get_export(&export_id)
            .await
            .map_err(tool_err)?;
        rec.status = crate::store::ExportStatus::Running;
        self.shared.store.update_export(&rec).await.map_err(tool_err)?;

        match self.shared.exporter.export(job).await {
            Ok(out) => {
                let size = out.bytes.len() as i64;
                let key = crate::assets::export_blob_key(&input.deck_id, &export_id, format.as_str());
                self.shared
                    .exports
                    .put(&key, out.bytes)
                    .await
                    .map_err(tool_err)?;
                rec.status = crate::store::ExportStatus::Done;
                rec.artifact_key = Some(key);
                rec.artifact_mime = Some(out.mime);
                rec.artifact_size = Some(size);
                rec.error = None;
            }
            Err(e) => {
                rec.status = crate::store::ExportStatus::Error;
                rec.error = Some(e.to_string());
            }
        }
        self.shared.store.update_export(&rec).await.map_err(tool_err)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "export_id": export_id,
            "deck_id": input.deck_id,
            "format": format.as_str(),
            "status": rec.status.as_str(),
            "pages": pages.iter().map(|p| p + 1).collect::<Vec<_>>(),
            "dpi": dpi,
            "error": rec.error,
        })))
    }
}
