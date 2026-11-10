//! Asset tools: attach figures/images to decks with caption + provenance.
//!
//! Bytes come from a REAL source — never generated inline by the model:
//!
//! - `vfs_path` (preferred): read via the process-wide VFS. DAG node
//!   outputs, container artifacts, and code-generated figures all land
//!   on the VFS, so one path reference covers every workflow
//!   (`/data/figures/fig1.png`, container work-dir outputs, …).
//! - `data_base64`: small payloads only (tests, tiny diagrams). Models
//!   must not synthesize image bytes; anything beyond a few KB belongs
//!   on the VFS.

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use std::sync::Arc;

use super::{DeckToolState, box_write, tool_err};
use crate::actor::DeckHandle;
use crate::shared::DeckShared;

/// 50 MiB — same cap as the HTTP multipart route.
const MAX_ASSET_BYTES: usize = 50 * 1024 * 1024;
/// Inline base64 payloads beyond this are rejected with VFS guidance.
const MAX_INLINE_BYTES: usize = 256 * 1024;

/// Build asset tool registrations.
pub fn registrations(state: DeckToolState) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(AssetAttachTool {
        handle: state.handle,
    })]
}

// ===========================================================================
// asset_attach
// ===========================================================================

#[tool(
    name = "asset_attach",
    description = "Attach an image/figure file to a deck from a REAL source. Assets are \
                   referenced in slide content/frontmatter as `assets://{asset_id}` and \
                   resolved to URLs by the renderer/exporter. Captions feed figure layouts; \
                   provenance records the data lineage (e.g. dag://run/<id>/<node>). \
                  \
                  **Preferred**: pass `vfs_path` — the VFS path where the bytes already exist \
                  (DAG node output, container artifact, plot file written by code). \
                  `data_base64` is for small (<256KB) payloads only; never synthesize image \
                  bytes inline. Pass exactly one source. \
                  \
                  **Examples**: \
                  • deck_id=\"deck-1\", vfs_path=\"/data/run42/km_plot.png\", \
                    caption=\"Figure 1. Kaplan-Meier curves\", provenance=\"dag://run/42/km_plot\" \
                  • deck_id=\"deck-1\", filename=\"seq.png\", data_base64=\"iVBORw0KGgo…\""
)]
pub struct AssetAttachInput {
    #[desc = "Deck ID"]
    pub deck_id: String,
    #[desc = "VFS path of the existing file (preferred source), e.g. /data/figures/fig1.png"]
    pub vfs_path: Option<String>,
    #[desc = "Base64-encoded bytes (small payloads <256KB only)"]
    pub data_base64: Option<String>,
    #[desc = "Display filename (default: basename of vfs_path)"]
    pub filename: Option<String>,
    #[desc = "Figure caption (rendered in the caption slot of figure layouts)"]
    pub caption: Option<String>,
    #[desc = "Data provenance note (DAG node output, file path, or source)"]
    pub provenance: Option<String>,
}

struct AssetAttachTool {
    handle: DeckHandle,
}

#[async_trait]
impl ToolFunction for AssetAttachTool {
    type Input = AssetAttachInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Whole attach (deck check + source read + blob put + meta row)
        // serializes through the write actor.
        let (meta, bytes_len) = self
            .handle
            .write({
                let deck_id = input.deck_id.clone();
                let vfs_path = input.vfs_path.clone();
                let data_base64 = input.data_base64.clone();
                let filename_in = input.filename.clone();
                let caption = input.caption.clone();
                let provenance = input.provenance.clone();
                move |s: Arc<DeckShared>| {
                    box_write(async move {
                        attach_inner(
                            s, deck_id, vfs_path, data_base64, filename_in, caption, provenance,
                        )
                        .await
                    })
                }
            })
            .await
            .map_err(tool_err)?;
        let bytes_len = bytes_len as usize;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "asset_id": meta.id,
            "ref": format!("assets://{}", meta.id),
            "filename": meta.filename,
            "mime": meta.mime,
            "size": bytes_len,
            "blob_key": meta.blob_key,
            "usage": "reference as assets://{asset_id} in slide content or frontmatter image",
        })))
    }
}

/// The serialized attach body (runs on the deck write worker).
#[allow(clippy::too_many_arguments)]
async fn attach_inner(
    s: Arc<DeckShared>,
    deck_id: String,
    vfs_path: Option<String>,
    data_base64: Option<String>,
    filename_in: Option<String>,
    caption: Option<String>,
    provenance: Option<String>,
) -> crate::Result<(crate::store::AssetMeta, i64)> {
    // Validate the deck exists.
    s.store.get_deck_row(&deck_id).await?;

    // Exactly one source.
    let (bytes, filename) = match (&vfs_path, &data_base64) {
        (Some(_), Some(_)) => {
            return Err(crate::Error::Asset(
                "pass exactly one source: vfs_path or data_base64".into(),
            ));
        }
        (None, None) => {
            return Err(crate::Error::Asset(
                "no source: pass vfs_path (preferred) or data_base64".into(),
            ));
        }
        (Some(path), None) => {
            let bytes = read_vfs(&s, path).await?;
            let name = filename_in.clone().unwrap_or_else(|| basename(path));
            (bytes, name)
        }
        (None, Some(b64)) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| {
                    crate::Error::Asset(format!(
                        "invalid base64: {e} (note: for real files use vfs_path instead)"
                    ))
                })?;
            if bytes.len() > MAX_INLINE_BYTES {
                return Err(crate::Error::Asset(format!(
                    "inline payload too large ({} bytes > {MAX_INLINE_BYTES}); \
                         write the file to the VFS and pass vfs_path instead",
                    bytes.len()
                )));
            }
            let name = filename_in.clone().ok_or_else(|| {
                crate::Error::Asset("filename is required when using data_base64".into())
            })?;
            (bytes, name)
        }
    };

        if bytes.is_empty() {
            return Err(crate::Error::Asset("empty asset payload".into()));
        }
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(crate::Error::Asset(format!(
                "asset too large: {} bytes (cap {MAX_ASSET_BYTES})",
                bytes.len()
            )));
        }

        let asset_id = format!("asset-{}", uuid::Uuid::new_v4().simple());
        let blob_key = crate::assets::asset_blob_key(&deck_id, &asset_id, &filename);
        s.assets.put(&blob_key, bytes.clone()).await?;

        let sha = {
            let mut h = Sha256::new();
            h.update(&bytes);
            format!("{:x}", h.finalize())
        };
        let mime = guess_mime(&filename);
        let meta = crate::store::AssetMeta {
            id: asset_id.clone(),
            deck_id: deck_id.clone(),
            filename: filename.clone(),
            mime: mime.into(),
            size: bytes.len() as i64,
            sha256: sha,
            caption,
            provenance,
            blob_key: blob_key.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        s.store.add_asset(&meta).await?;
        Ok((meta, bytes.len() as i64))
}

/// Read bytes from the shared VFS (mount-routed; the sanctioned API,
/// no raw filesystem access).
async fn read_vfs(s: &DeckShared, vpath: &str) -> crate::Result<Vec<u8>> {
    #[cfg(not(feature = "vfs"))]
    {
        return Err(crate::Error::Asset(
            "no VFS support in this build; pass data_base64 instead".into(),
        ));
    }
    #[cfg(feature = "vfs")]
    {
        let storage = s.vfs.as_ref().ok_or_else(|| {
            crate::Error::Asset(
                "no VFS attached in this deployment; pass data_base64 instead".into(),
            )
        })?;
        let normalized = storage
            .try_resolve_virtual(vpath)
            .map_err(|e| crate::Error::Asset(format!("invalid VFS address {vpath}: {e}")))?;
        let operator = storage.resolve(&normalized);
        let path = storage.resolve_path(&normalized);
        let buffer = operator.read(&path).await.map_err(|e| {
            crate::Error::Asset(format!(
                "VFS read failed for {vpath}: {e} (check the path exists — e.g. ls it via the file tools first)"
            ))
        })?;
        Ok(buffer.to_vec())
    }
}

fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let name = trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("asset.bin");
    #[cfg(feature = "vfs")]
    if vfs::uri::domain(path).is_some() {
        return vfs::uri::decode_path(name).unwrap_or_else(|_| name.to_string());
    }
    name.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "vfs")]
    #[tokio::test]
    async fn domain_asset_reads_preserve_escaped_names_and_refuse_unknown_domains() {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        storage
            .write_bytes("local://figure%20%25.png", b"image".to_vec())
            .await
            .unwrap();
        assert_eq!(basename("local://figure%20%25.png"), "figure %.png");
        assert_eq!(basename("/figure%20.png"), "figure%20.png");
        let shared = DeckShared::open_in_memory()
            .await
            .unwrap()
            .with_vfs(Some(storage));
        assert_eq!(
            read_vfs(&shared, "local://figure%20%25.png")
                .await
                .unwrap(),
            b"image"
        );
        assert!(read_vfs(&shared, "missing://figure.png").await.is_err());
    }

    #[test]
    fn basename_helpers() {
        assert_eq!(basename("/data/figures/fig1.png"), "fig1.png");
        assert_eq!(basename("trailing/"), "trailing");
        assert_eq!(basename("plain.png"), "plain.png");
    }
}
