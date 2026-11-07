//! Asset tools: attach figures/images to decks with caption + provenance.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::{tool_err, DeckToolState};
use crate::shared::DeckShared;

/// Build asset tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(AssetAttachTool {
        shared: state.shared,
    })]
}

// ===========================================================================
// asset_attach
// ===========================================================================

#[tool(
    name = "asset_attach",
    description = "Attach an image/figure to a deck (base64 bytes). Assets are referenced in \
                   slide content/frontmatter as `assets://{asset_id}` and resolved to URLs by \
                   the renderer/exporter. Captions feed figure layouts; provenance records the \
                   data lineage (e.g. dag://run/<id>/<node> or a file path). \
                  \
                  **Examples**: \
                  • deck_id=\"deck-1\", filename=\"fig1.png\", data_base64=\"iVBORw0KGgo…\", \
                    caption=\"Figure 1. Kaplan-Meier curves\", provenance=\"dag://run/42/km_plot\""
)]
pub struct AssetAttachInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "Original filename (extension sets the blob type)"]
    pub filename: String,
    #[desc = "Base64-encoded file bytes"]
    pub data_base64: String,
    #[desc = "Figure caption (rendered in the caption slot of figure layouts)"]
    pub caption: Option<String>,
    #[desc = "Data provenance note (DAG node output, file path, or source)"]
    pub provenance: Option<String>,
}

struct AssetAttachTool {
    shared: DeckShared,
}

#[async_trait]
impl ToolFunction for AssetAttachTool {
    type Input = AssetAttachInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Validate the deck exists.
        let row = self
            .shared
            .store
            .get_deck_row(&input.deck_id)
            .await
            .map_err(tool_err)?;
        let _ = &row;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input.data_base64.trim())
            .map_err(|e| agentik_core::tools::ToolError::ExecutionFailed {
                source: Box::new(crate::Error::Asset(format!("invalid base64: {e}"))),
            })?;
        if bytes.is_empty() {
            return Err(agentik_core::tools::ToolError::ExecutionFailed {
                source: Box::new(crate::Error::Asset("empty asset payload".into())),
            });
        }

        let asset_id = format!("asset-{}", uuid::Uuid::new_v4().simple());
        let blob_key = crate::assets::asset_blob_key(&input.deck_id, &asset_id, &input.filename);
        self.shared
            .assets
            .put(&blob_key, bytes.clone())
            .await
            .map_err(tool_err)?;

        let sha = {
            let mut h = Sha256::new();
            h.update(&bytes);
            format!("{:x}", h.finalize())
        };
        let mime = guess_mime(&input.filename);
        let meta = crate::store::AssetMeta {
            id: asset_id.clone(),
            deck_id: input.deck_id.clone(),
            filename: input.filename.clone(),
            mime: mime.into(),
            size: bytes.len() as i64,
            sha256: sha,
            caption: input.caption.clone(),
            provenance: input.provenance.clone(),
            blob_key: blob_key.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.shared.store.add_asset(&meta).await.map_err(tool_err)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "asset_id": asset_id,
            "ref": format!("assets://{asset_id}"),
            "filename": input.filename,
            "mime": mime,
            "size": bytes.len(),
            "blob_key": blob_key,
        })))
    }
}

/// Guess a mime type from a filename extension (small allowlist for
/// figure formats).
pub fn guess_mime(filename: &str) -> &'static str {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}
