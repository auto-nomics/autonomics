use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::UniProtClient;
use crate::format::format_id_mapping;

#[tool(
    name = "uniprot_idmap",
    description = "Map protein/gene identifiers between databases via the UniProt ID \
                  mapping service (runs server-side, may take a few seconds). \
                  \
                  Common `from`/`to` database names: UniProtKB_AC-ID, Acc, \
                  Gene_Name, Ensembl, Ensembl_Genomes, RefSeq_Protein, Entrez_Gene \
                  (GeneID), PDB, EMBL, HGNC. \
                  \
                  Examples: \
                  - Ensembl transcript IDs → UniProt: from=\"Ensembl\", \
                    to=\"UniProtKB\", ids=[\"ENST00000397029\"] \
                  - gene symbols → UniProt accessions: from=\"Gene_Name\", \
                    to=\"UniProtKB\", ids=[\"INS\", \"GCG\"] \
                  \
                  Output: 'markdown' table (default) or 'tsv' raw text with a \
                  From column plus UniProtKB fields."
)]
pub struct UniprotIdmapInput {
    #[desc = "Source database name, e.g. \"Ensembl\", \"Gene_Name\", \"RefSeq_Protein\"."]
    pub from: String,

    #[desc = "Target database name, e.g. \"UniProtKB\", \"UniProtKB-Swiss-Prot\", \
             \"Gene_Name\"."]
    pub to: String,

    #[desc = "IDs to map (up to 100,000 per job; keep batches modest)."]
    pub ids: Vec<String>,

    #[desc = "Output format: 'markdown' table (default) or 'tsv'."]
    pub output: Option<String>,
}

pub struct UniprotIdmapTool {
    pub(crate) client: Arc<UniProtClient>,
}

#[async_trait]
impl ToolFunction for UniprotIdmapTool {
    type Input = UniprotIdmapInput;

    fn timeout_seconds(&self) -> u64 {
        300
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.ids.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "uniprot_idmap requires at least one `ids` entry".into(),
            });
        }

        let from = input.from.trim();
        let to = input.to.trim();
        if from.is_empty() || to.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "uniprot_idmap requires non-empty `from` and `to`".into(),
            });
        }

        let text = match input.output.as_deref().unwrap_or("markdown") {
            "tsv" => {
                let fields: Vec<String> = ["accession", "id", "protein_name", "organism_name"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
                self.client
                    .map_ids(from, to, &input.ids, Some(&fields))
                    .await
                    .map_err(super::json_err)?
            }
            _ => {
                let results = self
                    .client
                    .map_ids_json(from, to, &input.ids)
                    .await
                    .map_err(super::json_err)?;
                format_id_mapping(&results)
            }
        };

        Ok(AgentToolResult::success(text))
    }
}
