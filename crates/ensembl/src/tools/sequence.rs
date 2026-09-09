use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::SequenceType;
use crate::format::format_sequence;

#[tool(
    name = "ensembl_sequence",
    description = "Preview genomic, cDNA, CDS, or protein sequence for an Ensembl stable ID. Long sequences are summarized rather than returned in full."
)]
pub struct EnsemblSequenceInput {
    #[desc = "Ensembl gene, transcript, translation, or exon ID, e.g. 'ENST00000380152'."]
    pub id: String,

    #[desc = "Sequence type: 'dna' (default), 'cdna', 'cds', or 'protein'."]
    pub sequence_type: Option<SequenceType>,

    #[desc = "Bases to add upstream. Not valid for every sequence type."]
    pub expand_5prime: Option<u64>,

    #[desc = "Bases to add downstream. Not valid for every sequence type."]
    pub expand_3prime: Option<u64>,
}

pub struct EnsemblSequenceTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblSequenceTool {
    type Input = EnsemblSequenceInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let sequence = self
            .client
            .sequence_id(
                &input.id,
                input.sequence_type.unwrap_or(SequenceType::Dna),
                input.expand_5prime,
                input.expand_3prime,
            )
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_sequence(&sequence)))
    }
}
