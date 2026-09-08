use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::UniProtClient;
use crate::format::format_entry;
use crate::types::Format;

#[tool(
    name = "uniprot_entry",
    description = "Fetch one UniProtKB protein entry by accession (e.g. P01308, P0DTC2, \
                  or an isoform like P01308-2). \
                  \
                  Output modes: \
                  - 'markdown' (default): full annotation — names, gene, organism, \
                    function and other comments, keywords, cross-references, sequence stats. \
                  - 'fasta': the amino-acid sequence in FASTA format. \
                  - 'txt': the complete UniProt flat file (everything, raw). \
                  - 'gff': sequence features (domains, variants, PTMs) in GFF3."
)]
pub struct UniprotEntryInput {
    #[desc = "UniProt accession, e.g. \"P01308\" (human insulin)."]
    pub accession: String,

    #[desc = "Output format: 'markdown' (default), 'fasta', 'txt', or 'gff'."]
    pub output: Option<String>,
}

pub struct UniprotEntryTool {
    pub(crate) client: Arc<UniProtClient>,
}

#[async_trait]
impl ToolFunction for UniprotEntryTool {
    type Input = UniprotEntryInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let output = input.output.as_deref().unwrap_or("markdown");
        let accession = input.accession.trim();
        if accession.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "uniprot_entry requires a non-empty `accession`".into(),
            });
        }

        let text = match output.to_ascii_lowercase().as_str() {
            "markdown" | "md" | "" => {
                let entry = self
                    .client
                    .entry(accession)
                    .await
                    .map_err(super::json_err)?;
                format_entry(&entry)
            }
            other => {
                let format = Format::parse(other).ok_or_else(|| ToolError::ValidationFailed {
                    message: format!(
                        "unknown output format {other:?}: expected markdown, \
                             fasta, txt or gff"
                    ),
                })?;
                self.client
                    .entry_text(accession, format)
                    .await
                    .map_err(super::json_err)?
            }
        };

        Ok(AgentToolResult::success(text))
    }
}
