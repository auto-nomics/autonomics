//! Slide operation tools — the Slidev MCP patch model exposed to agents.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use std::sync::Arc;

use super::{box_write, tool_err, DeckToolState};
use crate::actor::DeckHandle;
use crate::shared::DeckShared;
use crate::operations::SlidePatch;

/// Build slide-operation tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SlidePatchTool { handle: state.handle.clone() }),
        ToolRegistration::from(SlideInsertTool { handle: state.handle.clone() }),
        ToolRegistration::from(SlideRemoveTool { handle: state.handle.clone() }),
        ToolRegistration::from(SlideMoveTool { handle: state.handle }),
    ]
}

async fn outline_delta(shared: &DeckShared, deck_id: &str) -> crate::Result<serde_json::Value> {
    let deck = shared.load_deck(deck_id).await?;
    Ok(serde_json::json!({
        "revision": deck.meta.revision,
        "slide_count": deck.len(),
        "diagnostics": crate::diagnostics::write_hook(shared, &deck),
        "outline": deck.slides.iter().map(|s| serde_json::json!({
            "no": s.no(),
            "title": s.title,
            "layout": s.frontmatter.layout.clone().map(|l| l.to_string()),
            "clicksTotal": s.click_plan.as_ref().map(|p| p.total),
        })).collect::<Vec<_>>(),
    }))
}

// ===========================================================================
// slide_patch
// ===========================================================================

#[tool(
    name = "slide_patch",
    description = "Patch one slide of a deck (1-based `no`). Replace the body content, the \
                   speaker note (\"\" clears), or the frontmatter YAML (raw text, \"\" clears — \
                   forbidden on slide 1 whose frontmatter is the deck headmatter). \
                   Returns the new deck revision and outline. \
                  \
                  **Examples**: \
                  • deck_id=\"deck-1\", no=3, content=\"# Results\\n\\n- point\" \
                  • deck_id=\"deck-1\", no=2, note=\"强调置信区间\" \
                  • deck_id=\"deck-1\", no=4, frontmatter_raw=\"layout: figure-left\\nimage: assets://a1\""
)]
pub struct SlidePatchInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based slide number"]
    pub no: u32,
    #[desc = "New body markdown (optional — omit to keep)"]
    pub content: Option<String>,
    #[desc = "New frontmatter YAML as raw text; \"\" clears it (slide 1 forbidden)"]
    pub frontmatter_raw: Option<String>,
    #[desc = "New speaker note; \"\" clears (optional)"]
    pub note: Option<String>,
}

struct SlidePatchTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for SlidePatchTool {
    type Input = SlidePatchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck_id = input.deck_id.clone();
        let patch_no = input.no;
        let deck_id_out = deck_id.clone();
        let mut out = self
            .handle
            .write(move |s: Arc<DeckShared>| {
                box_write(async move {
                    let row = s.store.get_deck_row(&deck_id).await?;
                    let md = crate::operations::apply_slide_patch(
                        &row.markdown,
                        &deck_id,
                        patch_no as usize,
                        SlidePatch {
                            content: input.content,
                            frontmatter_raw: input.frontmatter_raw,
                            note: input.note,
                        },
                    )?;
                    s.save_markdown(&deck_id, &md, &format!("patch slide {patch_no}")).await?;
                    outline_delta(&s, &deck_id).await
                })
            })
            .await
            .map_err(tool_err)?;
        out["deck_id"] = serde_json::Value::String(deck_id_out);
        out["patched"] = serde_json::json!(patch_no);
        Ok(AgentToolResult::success_json(out))
    }
}

// ===========================================================================
// slide_insert
// ===========================================================================

#[tool(
    name = "slide_insert",
    description = "Insert a new slide after 1-based position `after` (0 = prepend as slide 1 — \
                  note slide 1 carries the deck headmatter, so prefer after>=1). Body content, \
                  frontmatter YAML, and speaker note are all optional. Returns the new slide no."
)]
pub struct SlideInsertInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "Insert after this 1-based slide (0 = prepend)"]
    pub after: u32,
    #[desc = "Body markdown (optional)"]
    pub content: Option<String>,
    #[desc = "Frontmatter YAML as raw text (optional)"]
    pub frontmatter_raw: Option<String>,
    #[desc = "Speaker note (optional)"]
    pub note: Option<String>,
}

struct SlideInsertTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for SlideInsertTool {
    type Input = SlideInsertInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck_id = input.deck_id.clone();
        let after = input.after;
        let deck_id_out = deck_id.clone();
        let mut out = self
            .handle
            .write(move |s: Arc<DeckShared>| {
                box_write(async move {
                    let row = s.store.get_deck_row(&deck_id).await?;
                    let md = crate::operations::insert_slide(
                        &row.markdown,
                        &deck_id,
                        after as usize,
                        input.content.as_deref().unwrap_or(""),
                        input.frontmatter_raw.as_deref(),
                        input.note.as_deref(),
                    )?;
                    s.save_markdown(&deck_id, &md, &format!("insert after {after}")).await?;
                    let mut out = outline_delta(&s, &deck_id).await?;
                    out["inserted_no"] = serde_json::json!(after as usize + 1);
                    Ok(out)
                })
            })
            .await
            .map_err(tool_err)?;
        out["deck_id"] = serde_json::Value::String(deck_id_out);
        Ok(AgentToolResult::success_json(out))
    }
}

// ===========================================================================
// slide_remove
// ===========================================================================

#[tool(
    name = "slide_remove",
    description = "Remove a slide (1-based; slide 1 is the headmatter and cannot be removed). \
                   Returns the remaining outline."
)]
pub struct SlideRemoveInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based slide number to remove"]
    pub no: u32,
}

struct SlideRemoveTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for SlideRemoveTool {
    type Input = SlideRemoveInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck_id = input.deck_id.clone();
        let no = input.no;
        let deck_id_out = deck_id.clone();
        let mut out = self
            .handle
            .write(move |s: Arc<DeckShared>| {
                box_write(async move {
                    let row = s.store.get_deck_row(&deck_id).await?;
                    let md = crate::operations::remove_slide(&row.markdown, &deck_id, no as usize)?;
                    s.save_markdown(&deck_id, &md, &format!("remove slide {no}")).await?;
                    let mut out = outline_delta(&s, &deck_id).await?;
                    out["removed"] = serde_json::json!(no);
                    Ok(out)
                })
            })
            .await
            .map_err(tool_err)?;
        out["deck_id"] = serde_json::Value::String(deck_id_out);
        Ok(AgentToolResult::success_json(out))
    }
}

// ===========================================================================
// slide_move
// ===========================================================================

#[tool(
    name = "slide_move",
    description = "Move a slide: take the slide at 1-based `from` and place it at `to` \
                   (both in the original numbering). Position 1 (headmatter) is fixed."
)]
pub struct SlideMoveInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "1-based source position"]
    pub from: u32,
    #[desc = "1-based target position"]
    pub to: u32,
}

struct SlideMoveTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for SlideMoveTool {
    type Input = SlideMoveInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck_id = input.deck_id.clone();
        let (from, to) = (input.from, input.to);
        let deck_id_out = deck_id.clone();
        let mut out = self
            .handle
            .write(move |s: Arc<DeckShared>| {
                box_write(async move {
                    let row = s.store.get_deck_row(&deck_id).await?;
                    let md = crate::operations::move_slide(&row.markdown, &deck_id, from as usize, to as usize)?;
                    s.save_markdown(&deck_id, &md, &format!("move {from} -> {to}")).await?;
                    let mut out = outline_delta(&s, &deck_id).await?;
                    out["moved"] = serde_json::json!({"from": from, "to": to});
                    Ok(out)
                })
            })
            .await
            .map_err(tool_err)?;
        out["deck_id"] = serde_json::Value::String(deck_id_out);
        Ok(AgentToolResult::success_json(out))
    }
}
