use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::{format_assembly, format_species};

#[tool(
    name = "ensembl_species",
    description = "Preview Ensembl species metadata. Optionally filter by a case-insensitive substring in scientific name, common name, alias, taxonomy ID, assembly, or division."
)]
pub struct EnsemblSpeciesInput {
    #[desc = "Case-insensitive filter, e.g. 'human', 'mouse', '9606', or 'GRCh38'."]
    pub query: Option<String>,
    #[desc = "Exact Ensembl division, e.g. 'EnsemblVertebrates'."]
    pub division: Option<String>,
    #[desc = "Maximum species shown in the summary (default 15)."]
    pub limit: Option<usize>,
}

pub struct EnsemblSpeciesTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblSpeciesTool {
    type Input = EnsemblSpeciesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let response = self.client.species().await.map_err(json_err)?;
        let mut species = response.species;
        if let Some(division) = input.division {
            species.retain(|item| item.division.eq_ignore_ascii_case(&division));
        }
        if let Some(query) = input.query {
            let query = query.to_ascii_lowercase();
            species.retain(|item| {
                let haystacks = [
                    Some(item.name.to_ascii_lowercase()),
                    item.display_name
                        .as_deref()
                        .map(|value| value.to_ascii_lowercase()),
                    item.common_name
                        .as_deref()
                        .map(|value| value.to_ascii_lowercase()),
                    Some(item.taxon_id.to_ascii_lowercase()),
                    item.assembly
                        .as_deref()
                        .map(|value| value.to_ascii_lowercase()),
                    Some(item.division.to_ascii_lowercase()),
                ];
                item.aliases
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&query))
                    || haystacks
                        .into_iter()
                        .flatten()
                        .any(|value| value.contains(&query))
            });
        }
        let limit = input.limit.unwrap_or(15);
        let shown = species
            .iter()
            .take(limit.min(species.len()))
            .cloned()
            .collect::<Vec<_>>();
        Ok(AgentToolResult::success(format_species(&shown)))
    }
}

#[tool(
    name = "ensembl_assembly",
    description = "Get assembly and coordinate-system metadata for an Ensembl species."
)]
pub struct EnsemblAssemblyInput {
    #[desc = "Ensembl species name, alias, common name, or taxonomy ID, e.g. 'human' or 'homo_sapiens'."]
    pub species: String,
}

pub struct EnsemblAssemblyTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblAssemblyTool {
    type Input = EnsemblAssemblyInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let assembly = self
            .client
            .assembly(&input.species)
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_assembly(&assembly)))
    }
}
