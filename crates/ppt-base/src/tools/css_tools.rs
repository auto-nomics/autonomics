//! Agent access to the separate theme file and per-slide CSS overrides.
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{box_write, tool_err, DeckToolState};
use crate::DeckHandle;

pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(DeckCssGetTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckCssSetTool { handle: state.handle }),
    ]
}

#[tool(name = "deck_css_get", description = "Read a presentation's theme.css, or a visible page's CSS override when no is supplied. Returns CSS and the revision required by deck_css_set. Use :scope for the slide root and .deck-prose for Markdown content; @import is not supported.")]
pub struct DeckCssGetInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based visible slide number; omit for the deck theme"]
    pub no: Option<u32>,
}
struct DeckCssGetTool { handle: DeckHandle }

#[async_trait]
impl ToolFunction for DeckCssGetTool {
    type Input = DeckCssGetInput;
    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let shared = self.handle.shared();
        let (css, revision) = if let Some(no) = input.no {
            let deck = shared.load_deck(&input.deck_id).await.map_err(tool_err)?;
            let slide = deck.slide(no as usize).ok_or_else(|| tool_err(crate::Error::SlideOutOfRange { deck: input.deck_id.clone(), no: no as usize, total: deck.len() }))?;
            let css = slide.frontmatter.extras.get("slideCss").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            (css, deck.meta.revision)
        } else {
            let css = shared.theme_css(&input.deck_id).await.map_err(tool_err)?;
            let revision = crate::shared::css_revision(&css);
            (css, revision)
        };
        Ok(AgentToolResult::success_json(serde_json::json!({ "deck_id": input.deck_id, "no": input.no, "css": css, "revision": revision })))
    }
}

#[tool(name = "deck_css_set", description = "Replace theme.css for a deck, or a visible page's CSS override if no is supplied. Empty CSS clears it. Supply the revision returned by deck_css_get to prevent overwriting concurrent edits. Preview and PDF/PNG/PPTX export use the same styles.")]
pub struct DeckCssSetInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based visible slide number; omit for the deck theme"]
    pub no: Option<u32>,
    #[desc = "CSS source; empty clears the style"]
    pub css: String,
    #[desc = "Revision returned by deck_css_get"]
    pub revision: String,
}
struct DeckCssSetTool { handle: DeckHandle }

#[async_trait]
impl ToolFunction for DeckCssSetTool {
    type Input = DeckCssSetInput;
    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let id = input.deck_id.clone();
        let no = input.no;
        let revision = self.handle.write(move |s| box_write(async move {
            if let Some(no) = no {
                let row = s.store.get_deck_row(&id).await?;
                if row.revision != input.revision {
                    return Err(crate::Error::RevisionConflict { expected: row.revision, actual: input.revision });
                }
                let markdown = crate::operations::set_slide_css(&row.markdown, &id, no as usize, &input.css)?;
                Ok(s.save_markdown(&id, &markdown, "set slide CSS").await?.meta.revision)
            } else {
                s.save_theme_css(&id, &input.css, &input.revision).await
            }
        })).await.map_err(tool_err)?;
        // Same write-hook contract as the slide tools: static findings for
        // the saved state + render-tier scheduling status.
        let diagnostics = {
            let shared = self.handle.shared();
            shared
                .load_deck(&input.deck_id)
                .await
                .map(|deck| crate::diagnostics::write_hook(shared, &deck))
                .unwrap_or_else(|e| serde_json::json!({
                    "renderTier": "unavailable",
                    "staticFindings": [],
                    "note": format!("diagnostics unavailable: {e}"),
                }))
        };
        Ok(AgentToolResult::success_json(serde_json::json!({ "deck_id": input.deck_id, "no": no, "revision": revision, "diagnostics": diagnostics })))
    }
}
