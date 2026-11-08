//! Deck management tools: create, list, get, outline, delete.
//!
//! Writes go through the [`DeckHandle`] actor (serialized); reads are
//! direct on the shared engine.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{tool_err, DeckToolState};
use crate::actor::DeckHandle;
use crate::shared::DeckShared;

/// Build deck-management tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(DeckCreateTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckListTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckGetTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckOutlineTool { handle: state.handle.clone() }),
        ToolRegistration::from(DeckDeleteTool { handle: state.handle }),
    ]
}

// ===========================================================================
// deck_create
// ===========================================================================

#[tool(
    name = "deck_create",
    description = "Create a new slide deck (presentation) from a biomedical template. \
                  Returns deck_id plus the starter outline — edit slides with slide_patch/slide_insert. \
                  \
                  **Templates**: \
                  • \"meeting\" — 组会/文献报告 (background, methods, panel-grid results, references) \
                  • \"defense\" — 答辩/会议 (click-reveal, stats-table, figure layouts, Q&A) \
                  • \"figures\" — 投稿配图/PPT 交付 (figure-full/figure-right/panel-grid, 300-DPI ready) \
                  • omit for a blank cover-only deck. \
                  \
                  **Examples**: \
                  • title=\"组会:单细胞图谱解析\", template=\"meeting\" \
                  • title=\"博士答辩\", template=\"defense\""
)]
pub struct DeckCreateInput {
    #[desc = "Deck title (headmatter title, shown on the cover slide)"]
    pub title: String,
    #[desc = "Starter template: \"meeting\", \"defense\", \"figures\", or omit for blank"]
    pub template: Option<String>,
    #[desc = "Aspect ratio: \"16/9\" (default), \"4/3\", \"1:1\"…"]
    pub aspect_ratio: Option<String>,
}

struct DeckCreateTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckCreateTool {
    type Input = DeckCreateInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck = self
            .handle
            .write({
                let title = input.title.clone();
                let template = input.template.clone();
                let aspect = input.aspect_ratio.clone();
                move |s: std::sync::Arc<DeckShared>| {
                    super::box_write(async move {
                        let mut deck = s.create_deck(&title, template.as_deref()).await?;
                        if let Some(ar) = aspect.as_deref() {
                            // Aspect change = headmatter edit; rebuild the source.
                            let row = s.store.get_deck_row(&deck.meta.id).await?;
                            let md =
                                row.markdown.replacen("aspectRatio: 16/9", &format!("aspectRatio: {ar}"), 1);
                            deck = s.save_markdown(&deck.meta.id, &md, "set aspect ratio").await?;
                            let initial = s.theme_css(&deck.meta.id).await?;
                            s.save_theme_css(&deck.meta.id, &crate::theme::starter_css(&deck.meta), &crate::shared::css_revision(&initial)).await?;
                        }
                        Ok(deck)
                    })
                }
            })
            .await
            .map_err(tool_err)?;

        let outline: Vec<serde_json::Value> = deck
            .slides
            .iter()
            .map(|s| {
                serde_json::json!({
                    "no": s.no(),
                    "title": s.title,
                    "layout": s.frontmatter.layout.clone().map(|l| l.to_string()),
                })
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "deck_id": deck.meta.id,
            "title": deck.meta.title,
            "slide_count": deck.len(),
            "aspect_ratio": deck.meta.aspect().to_string(),
            "features": deck.meta.features,
            "outline": outline,
        })))
    }
}

// ===========================================================================
// deck_list
// ===========================================================================

#[tool(
    name = "deck_list",
    description = "List slide decks in the library. Returns id, title, version, and timestamps."
)]
pub struct DeckListInput {
    #[desc = "Filter by title substring (optional)"]
    pub filter: Option<String>,
    #[desc = "Max rows (default 20)"]
    pub limit: Option<u32>,
}

struct DeckListTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckListTool {
    type Input = DeckListInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let shared = self.handle.shared().clone();
        let mut decks = shared.store.list_decks().await.map_err(tool_err)?;
        if let Some(f) = input.filter.as_deref().filter(|s| !s.is_empty()) {
            decks.retain(|d| d.title.contains(f));
        }
        let limit = input.limit.unwrap_or(20).max(1) as usize;
        decks.truncate(limit);
        let rows: Vec<serde_json::Value> = decks
            .iter()
            .map(|d| {
                serde_json::json!({
                    "deck_id": d.id,
                    "title": d.title,
                    "version": d.version,
                    "created_at": d.created_at,
                    "updated_at": d.updated_at,
                })
            })
            .collect();
        Ok(AgentToolResult::success_json(serde_json::json!({
            "decks": rows,
            "total": rows.len(),
        })))
    }
}

// ===========================================================================
// deck_get
// ===========================================================================

#[tool(
    name = "deck_get",
    description = "Get a deck: metadata, slide specs (titles, layouts, click plans, notes), \
                   asset list, and optionally the full canonical markdown source."
)]
pub struct DeckGetInput {
    #[desc = "Deck ID (from deck_create/deck_list)"]
    pub deck_id: String,
    #[desc = "Include the full markdown source (default true)"]
    pub include_markdown: Option<bool>,
}

struct DeckGetTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckGetTool {
    type Input = DeckGetInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let shared = self.handle.shared().clone();
        let deck = shared.load_deck(&input.deck_id).await.map_err(tool_err)?;
        let row = shared
            .store
            .get_deck_row(&input.deck_id)
            .await
            .map_err(tool_err)?;
        let assets = shared
            .store
            .list_assets(&input.deck_id)
            .await
            .map_err(tool_err)?;

        let include_md = input.include_markdown.unwrap_or(true);
        let slides: Vec<serde_json::Value> = deck
            .slides
            .iter()
            .map(|s| {
                serde_json::json!({
                    "no": s.no(),
                    "title": s.title,
                    "level": s.level,
                    "layout": s.frontmatter.layout.clone().map(|l| l.to_string()),
                    "clicksTotal": s.click_plan.as_ref().map(|p| p.total),
                    "hasNote": s.note.is_some(),
                    "revision": s.revision,
                })
            })
            .collect();
        let asset_rows: Vec<serde_json::Value> = assets
            .iter()
            .map(|a| {
                serde_json::json!({
                    "asset_id": a.id,
                    "filename": a.filename,
                    "mime": a.mime,
                    "size": a.size,
                    "caption": a.caption,
                    "provenance": a.provenance,
                    "ref": format!("assets://{}", a.id),
                })
            })
            .collect();

        let mut out = serde_json::json!({
            "deck": deck.meta,
            "slides": slides,
            "assets": asset_rows,
            "version": row.version,
        });
        if include_md {
            out["markdown"] = serde_json::Value::String(row.markdown);
        }
        Ok(AgentToolResult::success_json(out))
    }
}

// ===========================================================================
// deck_outline
// ===========================================================================

#[tool(
    name = "deck_outline",
    description = "Outline a deck: per-slide no / title / level / layout / click total / note \
                   presence / hidden state — for planning the narrative arc before editing."
)]
pub struct DeckOutlineInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
}

struct DeckOutlineTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckOutlineTool {
    type Input = DeckOutlineInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck = self
            .handle
            .shared()
            .load_deck(&input.deck_id)
            .await
            .map_err(tool_err)?;
        let hierarchy = crate::outline::build_outline(&deck.slides);
        let rows: Vec<serde_json::Value> = deck
            .slides
            .iter()
            .zip(hierarchy.iter())
            .map(|(s, node)| {
                serde_json::json!({
                    "no": s.no(),
                    "title": s.title,
                    "level": node.level,
                    "layout": s.frontmatter.layout.clone().map(|l| l.to_string()),
                    "clicksTotal": s.click_plan.as_ref().map(|p| p.total),
                    "hasNote": s.note.is_some(),
                    "depth": node.depth,
                    "parent": node.parent,
                    "isSection": node.is_section,
                    "sectionTitle": node.section_title,
                    "sectionPath": node.section_path,
                    "hideInToc": node.hidden,
                })
            })
            .collect();
        Ok(AgentToolResult::success_json(serde_json::json!({
            "deck_id": input.deck_id,
            "title": deck.meta.title,
            "slide_count": deck.len(),
            "outline": rows,
        })))
    }
}

// ===========================================================================
// deck_delete
// ===========================================================================

#[tool(
    name = "deck_delete",
    description = "Delete a slide deck and all its assets/exports permanently."
)]
pub struct DeckDeleteInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
}

struct DeckDeleteTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for DeckDeleteTool {
    type Input = DeckDeleteInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let deck_id = input.deck_id.clone();
        self.handle
            .write(move |s: std::sync::Arc<DeckShared>| {
                super::box_write(async move {
                    // blob cleanup best-effort, then the row cascade
                    let assets = s.store.list_assets(&deck_id).await?;
                    for a in &assets {
                        let _ = s.assets.delete(&a.blob_key).await;
                    }
                    let exports = s.store.list_exports(&deck_id).await?;
                    for e in &exports {
                        if let Some(key) = &e.artifact_key {
                            let _ = s.exports.delete(key).await;
                        }
                    }
                    let _ = s.assets.delete(&format!("themes/{deck_id}/theme.css")).await;
                    s.store.delete_deck(&deck_id).await?;
                    Ok(())
                })
            })
            .await
            .map_err(tool_err)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "deleted": input.deck_id,
        })))
    }
}
