use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use vfs::OpendalFileStorage;

use crate::ProtocolioClient;
use crate::query::ProtocolPdfView;
use crate::tools::search::error;

#[tool(
    name = "protocolio_download_pdf",
    description = "Download a protocols.io protocol as a PDF into virtual file storage. \
                  PDF generation is rate-limited independently by protocols.io."
)]
pub struct ProtocolioPdfInput {
    #[desc = "Numeric protocol ID or URI."]
    pub protocol_id: String,
    #[desc = "Destination path under VFS storage."]
    pub path: String,
    #[desc = "full, compact, materials, commands, or steps. Defaults to full."]
    pub view: Option<String>,
}

pub struct ProtocolioPdfTool {
    pub(crate) client: Arc<ProtocolioClient>,
    pub(crate) storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for ProtocolioPdfTool {
    type Input = ProtocolioPdfInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let view = input
            .view
            .as_deref()
            .map(ProtocolPdfView::parse)
            .transpose()
            .map_err(error)?
            .unwrap_or_default();
        if input.path.trim().is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "path must not be empty".to_owned(),
            });
        }
        let bytes = self
            .client
            .pdf(&input.protocol_id, view)
            .await
            .map_err(error)?;
        let size = bytes.len();
        self.storage
            .write_bytes(&input.path, bytes)
            .await
            .map_err(|err| error(crate::ProtocolioError::Storage(err.to_string())))?;
        Ok(AgentToolResult::success(format!(
            "Downloaded protocols.io PDF to {} ({} bytes).",
            input.path, size
        )))
    }
}
