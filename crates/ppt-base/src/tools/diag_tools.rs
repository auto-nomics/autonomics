//! Agent tool for the aesthetics-diagnostics system: read the latest
//! post-write report, or force a fresh run.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{tool_err, DeckToolState};
use crate::DeckHandle;

pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(DeckDiagnoseTool {
        handle: state.handle,
    })]
}

#[tool(
    name = "deck_diagnose",
    description = "Run aesthetics diagnostics on a deck and list every finding: text overflowing \
                   the canvas, clipped content, tiny fonts, low contrast, excessive text density, \
                   empty slides. Diagnostics also run automatically after every write; this tool \
                   returns the stored report when it is fresh, and otherwise re-runs (heavy: \
                   renders every slide in a real browser). Fix error-level findings before \
                   exporting."
)]
pub struct DeckDiagnoseInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "Force a fresh run even when the stored report is fresh"]
    pub refresh: Option<bool>,
}

struct DeckDiagnoseTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckDiagnoseTool {
    type Input = DeckDiagnoseInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let shared = self.handle.shared().clone();
        let report = if input.refresh.unwrap_or(false) {
            None
        } else {
            // Fresh stored report? Serve it without launching a browser.
            match shared.load_deck(&input.deck_id).await {
                Ok(deck) => {
                    let theme_revision = shared
                        .theme_css(&input.deck_id)
                        .await
                        .map(|css| crate::shared::css_revision(&css))
                        .unwrap_or_default();
                    match shared.latest_diagnostics(&input.deck_id).await {
                        Ok(Some(report))
                            if report.is_fresh(&deck.meta.revision, &theme_revision) =>
                        {
                            Some(report)
                        }
                        _ => None,
                    }
                }
                Err(e) => return Err(tool_err(e)),
            }
        };
        let report = match report {
            Some(report) => report,
            // Heavy synchronous run (browser); the caller asked for it.
            None => crate::diagnostics::run_diagnostics(&shared, &input.deck_id)
                .await
                .map_err(tool_err)?,
        };
        let (errors, warnings, infos) = report.counts();
        let text = crate::diagnostics::render_report_text(&report);
        Ok(AgentToolResult::success_json(serde_json::json!({
            "deck_id": input.deck_id,
            "revision": report.revision,
            "rendered": report.rendered,
            "errors": errors,
            "warnings": warnings,
            "infos": infos,
            "findings": report.findings,
            "summary": text.trim(),
        })))
    }
}
