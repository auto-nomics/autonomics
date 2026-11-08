//! Export tool: enqueue PDF/PNG/PPTX generation on the export queue.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use std::sync::Arc;

use super::{box_write, tool_err, DeckToolState};
use crate::actor::DeckHandle;
use crate::shared::DeckShared;

/// Build export tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(DeckExportTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckExportStatusTool { handle: state.handle }),
    ]
}

// ===========================================================================
// deck_export
// ===========================================================================

#[tool(
    name = "deck_export",
    description = "Export a deck to PDF (one page per slide, exact page size from the aspect \
                   ratio), PNG (zip of per-slide images, DPI-aware for journal figure specs), or \
                   PPTX (slides as full-bleed images + real speaker notes, editable page size). \
                   Runs on the export queue; poll the returned export_id with \
                   deck_export_status. \
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
    handle: DeckHandle,
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

        let deck_id = input.deck_id.clone();
        let deck_id_out = deck_id.clone();
        let range = input.range.clone();
        let dpi = input.dpi.unwrap_or(192).clamp(96, 300);
        let has_queue = self.handle.shared().export_queue.is_some();
        let (record, pages) = self
            .handle
            .write(move |s: Arc<DeckShared>| {
                box_write(async move {
                    let deck = s.load_deck(&deck_id).await?;
                    let pages = deck.resolve_range(range.as_deref())?;
                    let notes = pages
                        .iter()
                        .map(|p| deck.slide(p + 1).and_then(|sl| sl.note.clone()).unwrap_or_default())
                        .collect();
                    let canvas = s.canvas_size(&deck_id, &deck.meta).await?;
                    let (record, _token) = s
                        .enqueue_export(
                            &deck_id,
                            format,
                            dpi,
                            pages.iter().map(|p| p + 1).collect(),
                            canvas.width,
                            canvas.height,
                            notes,
                        )
                        .await?;
                    Ok((record, pages))
                })
            })
            .await
            .map_err(tool_err)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "export_id": record.id,
            "deck_id": deck_id_out,
            "format": format.as_str(),
            "status": record.status.as_str(),
            "pages": pages.iter().map(|p| p + 1).collect::<Vec<_>>(),
            "dpi": dpi,
            "hint": if has_queue {
                "queued; poll the export row for status"
            } else {
                "queued but no export worker is running in this process; the row stays queued"
            },
        })))
    }
}

// ===========================================================================
// deck_export_status
// ===========================================================================

#[tool(
    name = "deck_export_status",
    description = "Check an export job: status (queued/running/done/error), artifact size, and \
                   the error message on failure. Done artifacts download via \
                   GET /api/v1/deck/exports/{export_id}/download."
)]
pub struct DeckExportStatusInput {
    #[desc = "Export ID (from deck_export)"]
    pub export_id: String,
}

struct DeckExportStatusTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckExportStatusTool {
    type Input = DeckExportStatusInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let record = self
            .handle
            .shared()
            .store
            .get_export(&input.export_id)
            .await
            .map_err(tool_err)?;
        Ok(AgentToolResult::success_json(serde_json::json!({
            "export_id": record.id,
            "deck_id": record.deck_id,
            "format": record.format.as_str(),
            "status": record.status.as_str(),
            "artifact_size": record.artifact_size,
            "error": record.error,
        })))
    }
}
