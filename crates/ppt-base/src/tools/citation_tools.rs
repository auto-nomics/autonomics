//! Citation tool: generate/refresh the References slide from
//! `{{cite:key}}` markers against the shared bibliography.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::{tool_err, DeckToolState};
use crate::citation::{
    extract_cite_keys, format_slide_reference, references_slide_markdown, DeckCitationResolver,
};
use crate::operations::{apply_slide_patch, insert_slide, SlidePatch};
use crate::parser::parse;

#[tool(
    name = "deck_references",
    description = "Generate or refresh the deck's References slide: scans {{cite:key}} markers \
                   across all slides (or an explicit key list), resolves them against the shared \
                   bibliography, and writes a numbered `layout: references` slide — replacing the \
                   existing one when present, appending at the end otherwise. Unresolved keys are \
                   reported and kept out of the slide. \
                  \
                  **Example**: deck_id=\"deck-1\" (keys optional)"
)]
pub struct DeckReferencesInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "Explicit cite keys (default: scan {{cite:key}} markers in the deck)"]
    pub keys: Option<Vec<String>>,
}

struct DeckReferencesTool {
    shared: DeckToolStateShared,
}

type DeckToolStateShared = crate::shared::DeckShared;


#[async_trait]
impl ToolFunction for DeckReferencesTool {
    type Input = DeckReferencesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let bib = self.shared.bib.clone().ok_or_else(|| {
            agentik_core::tools::ToolError::ExecutionFailed {
                source: Box::new(crate::Error::Export(
                    "no bibliography attached to the deck engine in this deployment".into(),
                )),
            }
        })?;

        let row = self
            .shared
            .store
            .get_deck_row(&input.deck_id)
            .await
            .map_err(tool_err)?;

        let keys = match input.keys {
            Some(keys) if !keys.is_empty() => keys,
            _ => extract_cite_keys(&row.markdown),
        };
        if keys.is_empty() {
            return Ok(AgentToolResult::success_json(serde_json::json!({
                "deck_id": input.deck_id,
                "keys": 0,
                "message": "no {{cite:key}} markers found; pass an explicit key list to force generation",
            })));
        }

        let resolver = DeckCitationResolver::new(bib);
        let resolved = resolver.resolve_keys(&keys).await;
        let mut entries = Vec::new();
        let mut unresolved = Vec::new();
        for (key, article) in &resolved {
            match article {
                Some(a) => entries.push(format_slide_reference(a)),
                None => unresolved.push(key.clone()),
            }
        }

        let slide_md = references_slide_markdown(&entries);
        // Find the existing references slide (layout: references).
        let deck = parse(&row.markdown);
        let existing = deck
            .slides
            .iter()
            .rev()
            .find(|s| s.frontmatter.get("layout").and_then(|v| v.as_str()) == Some("references"));

        let message;
        let md = if let Some(existing) = existing {
            let no = existing.index + 1;
            message = format!("refresh references slide {no}");
            apply_slide_patch(
                &row.markdown,
                &input.deck_id,
                no,
                SlidePatch {
                    content: Some(slide_md),
                    frontmatter_raw: Some("layout: references".into()),
                    note: None,
                },
            )
        } else {
            message = "append references slide".to_string();
            insert_slide(
                &row.markdown,
                &input.deck_id,
                deck.slides.len(),
                &slide_md,
                Some("layout: references"),
                None,
            )
        }
        .map_err(tool_err)?;

        let deck_out = self
            .shared
            .save_markdown(&input.deck_id, &md, &message)
            .await
            .map_err(tool_err)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "deck_id": input.deck_id,
            "keys_requested": keys.len(),
            "resolved": entries.len(),
            "unresolved": unresolved,
            "slide_no": deck_out.len(),
            "slide_count": deck_out.len(),
            "revision": deck_out.meta.revision,
        })))
    }
}

/// Build citation tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(DeckReferencesTool {
        shared: state.shared,
    })]
}
