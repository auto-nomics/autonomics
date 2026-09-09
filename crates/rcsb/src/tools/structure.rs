use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::RcsbClient;
use crate::StructureFormat;

#[tool(
    name = "rcsb_structure_preview",
    description = "Fetch a bounded prefix of an RCSB text structure file (mmCIF, PDB, or FASTA) for quick inspection. Returns format, source URL, size, and the first records without loading the complete structure into context."
)]
pub struct RcsbStructurePreviewInput {
    #[desc = "Four-character PDB entry ID, e.g. '4HHB'."]
    pub entry_id: String,

    #[desc = "Text format: 'cif' or 'mmcif' (default), 'pdb', or 'fasta'."]
    pub format: Option<String>,

    #[desc = "Maximum bytes to download (default 32768, capped at 262144)."]
    pub max_bytes: Option<usize>,
}

pub struct RcsbStructurePreviewTool {
    pub client: Arc<RcsbClient>,
}

#[async_trait]
impl ToolFunction for RcsbStructurePreviewTool {
    type Input = RcsbStructurePreviewInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let format_name = input.format.as_deref().unwrap_or("cif");
        let format =
            StructureFormat::parse(format_name).ok_or_else(|| ToolError::ValidationFailed {
                message: format!(
                    "unknown structure format {format_name:?}; expected cif, pdb, or fasta"
                ),
            })?;
        let preview = self
            .client
            .structure_preview(&input.entry_id, format, input.max_bytes.unwrap_or(32_768))
            .await
            .map_err(super::tool_error)?;

        let mut markdown = crate::format::format_structure_preview(
            &preview.entry_id,
            preview.format.as_str(),
            &preview.url,
            preview.downloaded_bytes,
            &preview.text,
        );
        if let Some(total) = preview.total_bytes {
            markdown.push_str(&format!("\n\n- **Total file size:** {total} bytes\n"));
        }
        if preview.truncated {
            markdown.push_str("\n- **Preview:** truncated\n");
        }
        Ok(ToolResult::success(markdown))
    }
}
