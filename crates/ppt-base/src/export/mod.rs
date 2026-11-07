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
