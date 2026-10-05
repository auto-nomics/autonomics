//! Agent tools for KEGG data previews and information summaries.

// Tool Input structs marked `#[deprecated]` so the derived tool
// schema advertises `"deprecated": true` (survey T3 dual-track
// guidance); the module-local allow keeps the macro-generated
// impls in this file warning-free.
#![allow(deprecated)]

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::client::KeggClient;
use crate::error::KeggError;
use crate::format;
use crate::parser;

fn tool_error(error: KeggError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

fn normalize_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(20).clamp(1, 200)
}

#[tool(
    name = "kegg_info",
    description = "Get KEGG database, organism, pathway, or BRITE metadata including entry counts, release dates, and linked databases."
)]
pub struct KeggInfoInput {
    #[desc = "KEGG database name or organism code, e.g. 'kegg', 'pathway', 'ko', or 'hsa'."]
    pub database: Option<String>,
}

pub struct KeggInfoTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggInfoTool {
    type Input = KeggInfoInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let info = self
            .client
            .info(input.database.as_deref().unwrap_or("kegg"))
            .await
            .map_err(tool_error)?;
        Ok(AgentToolResult::success(format::format_info(&info)))
    }
}

#[tool(
    name = "kegg_find",
    description = "Search KEGG entries using native KEGG query syntax and return a concise identifier/description preview."
)]
pub struct KeggFindInput {
    #[desc = "Database to search, e.g. 'genes', 'hsa', 'compound', 'pathway', 'ko', or 'drug'."]
    pub database: Option<String>,
    #[desc = "KEGG query, e.g. 'BAIAP2', 'shiga toxin', or '\"shiga toxin\"'."]
    pub query: String,
    #[desc = "Optional native find option such as 'formula', 'exact_mass', or 'mol_weight'."]
    pub option: Option<String>,
    #[desc = "Maximum rows shown (1-200, default 20)."]
    pub limit: Option<usize>,
}

pub struct KeggFindTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggFindTool {
    type Input = KeggFindInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let database = input.database.as_deref().unwrap_or("genes");
        let items = self
            .client
            .find_with_option(database, &input.query, input.option.as_deref())
            .await
            .map_err(tool_error)?;
        Ok(AgentToolResult::success(format::format_entries(
            &items,
            normalize_limit(input.limit),
        )))
    }
}

#[tool(
    name = "kegg_entry_preview",
    description = "Preview one KEGG flat-file entry, including its type, organism, orthology, pathways, and cross-links."
)]
pub struct KeggEntryPreviewInput {
    #[desc = "One KEGG entry, e.g. 'hsa:10458', 'K05627', 'C00002', or 'hsa04151'."]
    pub entry: String,
    #[desc = "Maximum key header lines shown (1-100, default 20)."]
    pub limit: Option<usize>,
}

pub struct KeggEntryPreviewTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggEntryPreviewTool {
    type Input = KeggEntryPreviewInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let raw = self.client.get(&input.entry).await.map_err(tool_error)?;
        let entry = parser::flat_entry(&raw);
        Ok(AgentToolResult::success(format::format_entry(
            &entry,
            normalize_limit(input.limit),
        )))
    }
}

#[tool(
    name = "kegg_link",
    description = "Get KEGG database relationships, such as genes to pathways, KOs to reactions, or drugs to targets."
)]
pub struct KeggLinkInput {
    #[desc = "Target database, e.g. 'pathway', 'ko', 'rn', 'genes', or 'disease'."]
    pub target: String,
    #[desc = "Source database or entries, e.g. 'hsa', 'hsa:10458', or 'map00010'."]
    pub source: String,
    #[desc = "Maximum relationship rows shown (1-200, default 20)."]
    pub limit: Option<usize>,
}

pub struct KeggLinkTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggLinkTool {
    type Input = KeggLinkInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let pairs = self
            .client
            .link(&input.target, &input.source)
            .await
            .map_err(tool_error)?;
        Ok(AgentToolResult::success(format::format_pairs(
            "KEGG links",
            &pairs,
            normalize_limit(input.limit),
        )))
    }
}

#[tool(
    name = "kegg_convert",
    description = "Convert between KEGG IDs and integrated NCBI Gene, NCBI Protein, or UniProt identifiers."
)]
pub struct KeggConvertInput {
    #[desc = "Target database, e.g. 'ncbi-geneid', 'ncbi-proteinid', or 'uniprot'."]
    pub target: String,
    #[desc = "Source database or entries, e.g. 'hsa', 'eco', or 'hsa:10458'."]
    pub source: String,
    #[desc = "Maximum conversion rows shown (1-200, default 20)."]
    pub limit: Option<usize>,
}

pub struct KeggConvertTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggConvertTool {
    type Input = KeggConvertInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let pairs = self
            .client
            .conv(&input.target, &input.source)
            .await
            .map_err(tool_error)?;
        Ok(AgentToolResult::success(format::format_pairs(
            "KEGG ID conversions",
            &pairs,
            normalize_limit(input.limit),
        )))
    }
}

#[tool(
    name = "kegg_ddi",
    description = "Pipeline/dataframe use: prefer the DAG node `source_kegg_ddi` (typed table) — this tool stays for interactive lookup. Preview KEGG drug-drug interactions for one or more drug, compound, or drug-product identifiers."
)]
#[deprecated(note = "prefer the DAG node source_kegg_ddi for pipeline use")]
pub struct KeggDdiInput {
    #[desc = "One or more KEGG entries joined by '+', e.g. 'D00564' or 'D00564+D00100'."]
    pub entries: String,
    #[desc = "Maximum interaction rows shown (1-200, default 20)."]
    pub limit: Option<usize>,
}

pub struct KeggDdiTool {
    pub(crate) client: Arc<KeggClient>,
}

#[async_trait]
impl ToolFunction for KeggDdiTool {
    type Input = KeggDdiInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let interactions = self.client.ddi(&input.entries).await.map_err(tool_error)?;
        Ok(AgentToolResult::success(format::format_drug_interactions(
            &interactions,
            normalize_limit(input.limit),
        )))
    }
}

/// Build registrations for all KEGG agent tools.
pub fn kegg_registrations(client: Arc<KeggClient>) -> Vec<ToolRegistration> {
    use ToolRegistration as Registration;
    vec![
        Registration::from(KeggInfoTool {
            client: client.clone(),
        }),
        Registration::from(KeggFindTool {
            client: client.clone(),
        }),
        Registration::from(KeggEntryPreviewTool {
            client: client.clone(),
        }),
        Registration::from(KeggLinkTool {
            client: client.clone(),
        }),
        Registration::from(KeggConvertTool {
            client: client.clone(),
        }),
        Registration::from(KeggDdiTool { client }),
    ]
}
