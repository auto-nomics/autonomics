//! Preview tool: render one slide to PNG and hand it back to the agent
//! as an image block — the agent's only way to SEE the deck it is
//! editing (bib-base's figure-read pattern).

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{
    ToolResult as AgentToolResult, ToolResultBlock, ToolResultContent,
};
use async_trait::async_trait;
use base64::Engine as _;

use super::{tool_err, DeckToolState};
use crate::actor::DeckHandle;

/// Build preview tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(DeckSlidePreviewTool {
        handle: state.handle,
    })]
}

// ===========================================================================
// deck_slide_preview
// ===========================================================================

#[tool(
    name = "deck_slide_preview",
    description = "Render ONE slide of a deck to a PNG image and return it as an image block — \
                   look at what you built before exporting. Useful to check layout balance, \
                   figure placement, table fit, and CJK line breaks that raw markdown cannot \
                   reveal. Renders through the same engine as export (identical pixels). \
                  \
                  **Params**: `no` is the 1-based slide; `clicks` selects the reveal state \
                   (0 = before any v-click fires; omit for fully revealed). `dpi` defaults to \
                   144 (crisp but token-cheap; max 192). \
                  \
                  **Examples**: \
                  • deck_id=\"deck-1\", no=3 \
                  • deck_id=\"deck-1\", no=2, clicks=0 (check the pre-reveal state) \
                  • deck_id=\"deck-1\", no=5, dpi=192"
)]
pub struct DeckSlidePreviewInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based slide number to render"]
    pub no: u32,
    #[desc = "Click state to render (0 = pre-reveal; omit = fully revealed)"]
    pub clicks: Option<u32>,
    #[desc = "PNG DPI: default 144, max 192"]
    pub dpi: Option<u32>,
}

struct DeckSlidePreviewTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckSlidePreviewTool {
    type Input = DeckSlidePreviewInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let shared = self.handle.shared().clone();

        // Resolve the slide (read-only) for metadata + click clamping.
        let deck = shared.load_deck(&input.deck_id).await.map_err(tool_err)?;
        let no = input.no as usize;
        let slide = deck.slide(no).cloned().ok_or_else(|| {
            tool_err(crate::Error::SlideOutOfRange {
                deck: input.deck_id.clone(),
                no,
                total: deck.len(),
            })
        })?;
        let total_clicks = slide.click_plan.as_ref().map(|p| p.total).unwrap_or(0);
        let clicks = input.clicks.unwrap_or(total_clicks).min(total_clicks);
        let dpi = input.dpi.unwrap_or(144).clamp(96, 192);

        // Render outside the write lane (heavy: launches a browser); the
        // ephemeral token row is created and deleted inside preview_slide_png.
        let png = shared
            .preview_slide_png(&input.deck_id, no, clicks, dpi)
            .await
            .map_err(tool_err)?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let text = format!(
            "slide {no}/{} · {} · clicks {clicks}/{total_clicks} · {}×{}px @ {dpi}dpi ({:.0} KiB)",
            deck.len(),
            slide.title.as_deref().unwrap_or("(untitled)"),
            deck.meta.canvas(),
            deck.meta.canvas_height(),
            png.len() as f64 / 1024.0,
        );

        Ok(AgentToolResult {
            tool_use_id: String::new(),
            content: ToolResultContent::Blocks(vec![
                ToolResultBlock::text(text),
                ToolResultBlock::image_base64("image/png", b64),
            ]),
            is_error: None,
        })
    }
}
