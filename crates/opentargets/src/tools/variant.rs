use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::format::format_variant;
use crate::OpenTargetsClient;

#[tool(
    name = "opentargets_variant",
    description = "Get the annotation card for a variant by its Open Targets ID \
                  (chr_position_ref_alt, GRCh38). Returns alleles, position, linked \
                  rsIDs, and description."
)]
pub struct VariantInput {
    #[desc = "Variant ID in chr_pos_ref_alt format (GRCh38), e.g. '15_2835680_G_A'."]
    pub variant_id: String,
}

pub struct VariantTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for VariantTool {
    type Input = VariantInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let variant = self.client.variant(&input.variant_id).await.map_err(json_err)?;
        match variant {
            Some(v) => Ok(AgentToolResult::success(format_variant(&v))),
            None => Ok(AgentToolResult::error(format!(
                "No variant found for ID '{}'.",
                input.variant_id
            ))),
        }
    }
}
