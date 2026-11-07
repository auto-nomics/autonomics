//! Export backend trait + Null fallback.
//!
//! M8 wires the headless-Chromium exporter (print page + wait protocol +
//! `Page::pdf` / clip screenshots / OOXML PPTX); until then the
//! [`NullExporter`] records an actionable error on the export row, the
//! same pattern as writing-base's `NullEngine`.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One slide page request for image exports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub deck_id: String,
    pub export_id: String,
    pub format: crate::store::ExportFormat,
    /// PNG DPI (96 = 1x, up to 300).
    #[serde(default = "default_dpi")]
    pub dpi: u32,
    /// Page range string (`"1,4-5"`); empty = all.
    #[serde(default)]
    pub range: String,
    /// Loopback URL of the print page for this deck/export.
    pub print_url: String,
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
