//! Export backend trait + queue + Chromium implementation.

pub mod chromium;
pub mod pptx;
pub mod queue;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One export job (fully resolved: pages and print token included).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub deck_id: String,
    pub export_id: String,
    pub format: crate::store::ExportFormat,
    /// PNG DPI (96 = 1x, up to 300).
    #[serde(default = "default_dpi")]
    pub dpi: u32,
    /// 1-based page numbers to capture (subset via deck range string).
    #[serde(default)]
    pub pages: Vec<usize>,
    /// Canvas size in px (from deck meta at enqueue time).
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Speaker note per captured page (PPTX notes slides; empty = none).
    #[serde(default)]
    pub notes: Vec<String>,
    /// One-time print token minted at enqueue (raw; only the hash is
    /// persisted).
    #[serde(default)]
    pub print_token: String,
}

fn default_dpi() -> u32 {
    192
}

/// Output of a finished export.
#[derive(Debug, Clone)]
pub struct ExportOutput {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Export backend.
#[async_trait::async_trait]
pub trait DeckExporter: Send + Sync {
    /// Human-readable backend name.
    fn name(&self) -> &'static str;

    /// Run one export job, returning the artifact bytes.
    async fn export(&self, job: ExportJob) -> Result<ExportOutput>;

    /// Render ONE slide to PNG bytes for agent preview (synchronous —
    /// unlike export jobs this is not queued). Default: unsupported.
    async fn preview_png(
        &self,
        _job: &ExportJob,
        _slide_no: u32,
        _clicks: u32,
    ) -> Result<Vec<u8>> {
        Err(Error::Export(
            "this backend does not support slide preview".into(),
        ))
    }

    /// Aesthetics diagnostics: drive the print page to every
    /// `(slide, clicks)` target and evaluate `window.__deckDiagnose()`
    /// on each, returning a JSON object mapping slide number (string) to
    /// the findings array for that slide. Default: unsupported (the
    /// report degrades to the static tier).
    async fn diagnose_slides(
        &self,
        _job: &ExportJob,
        _targets: &[(u32, u32)],
    ) -> Result<serde_json::Value> {
        Err(Error::Export(
            "this backend does not support layout diagnostics".into(),
        ))
    }
}

/// No-op exporter: every job fails with an actionable message.
pub struct NullExporter;

#[async_trait::async_trait]
impl DeckExporter for NullExporter {
    fn name(&self) -> &'static str {
        "null"
    }

    async fn export(&self, _job: ExportJob) -> Result<ExportOutput> {
        Err(Error::Export(
            "no export backend available (headless Chromium exporter not enabled in this build)".into(),
        ))
    }
}
