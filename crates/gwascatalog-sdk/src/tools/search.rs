use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{client::GwasCatalogClient, format, search::SearchFilter};

#[tool(
    name = "gwascatalog_search",
    description = "Search the GWAS Catalog (EBI) via Solr full-text across all resource types \
                  — studies, variants, traits, genes, and publications. \
                  This is the most powerful discovery tool; use it first when unsure which \
                  resource applies. \
                  \
                  The `q` parameter uses Solr query syntax: \
                  • `*:*` matches everything \
                  • `resourcename:study AND accessionId:GCST005038` filters by resource + field \
                  • `\"breast cancer\"` for exact phrase \
                  • Boolean operators (uppercase): AND, OR, NOT \
                  • Range: `pvalue:[1e-8 TO 1e-5]` \
                  \
                  Filters (all optional): `pval_filter`, `or_filter`, `beta_filter`, \
                  `date_filter` accept range strings like `1e-8-1e-5` or `2020-2024`. \
                  `genomic_filter` uses `chrom:start-end` e.g. `1:1000000-2000000`. \
                  `trait_filter`, `genotyping_tech_filter`, `ancestry_filter` accept EFO URIs / \
                  labels."
)]
pub struct SearchInput {
    #[desc = "Solr query expression. Use `*:*` for all, or field-specific syntax \
             like `resourcename:study AND (\"breast cancer\")`."]
    pub q: String,
    #[desc = "Maximum results to return (default 20)."]
    pub size: Option<u32>,
    #[desc = "Offset for pagination (default 0)."]
    pub start: Option<u32>,
    #[desc = "P-value range filter, e.g. `1e-8-1e-5`."]
    pub pval_filter: Option<String>,
    #[desc = "Odds ratio range filter, e.g. `1.5-3.0`."]
    pub or_filter: Option<String>,
    #[desc = "Beta coefficient range filter, e.g. `0-1`."]
    pub beta_filter: Option<String>,
    #[desc = "Publication date range filter, e.g. `2020-2024`."]
    pub date_filter: Option<String>,
    #[desc = "Genomic location filter: `chrom:start-end`, e.g. `1:1000000-2000000`."]
    pub genomic_filter: Option<String>,
    #[desc = "EFO trait URI(s) to filter by, e.g. [\"EFO_0000400\"]."]
    pub trait_filter: Vec<String>,
    #[desc = "Genotyping technology filter(s)."]
    pub genotyping_tech_filter: Vec<String>,
    #[desc = "Ancestry label filter(s)."]
    pub ancestry_filter: Vec<String>,
}

pub struct SearchTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

#[async_trait]
impl ToolFunction for SearchTool {
    type Input = SearchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let filter = SearchFilter {
            q: input.q,
            max: input.size,
            start: input.start,
            pval_filter: input.pval_filter,
            or_filter: input.or_filter,
            beta_filter: input.beta_filter,
            date_filter: input.date_filter,
            genomic_filter: input.genomic_filter,
            trait_filter: input.trait_filter,
            genotyping_tech_filter: input.genotyping_tech_filter,
            ancestry_filter: input.ancestry_filter,
        };
        let resp = self.client.search(&filter).await.map_err(super::json_err)?;
        Ok(AgentToolResult::success(format::format_search(&resp)))
    }
}
