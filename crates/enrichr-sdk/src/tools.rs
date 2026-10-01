//! Agent tools for gene-set enrichment and library discovery.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EnrichrClient, format};

fn request_error(error: crate::EnrichrError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

fn validation(message: &str) -> ToolError {
    ToolError::ValidationFailed {
        message: message.to_owned(),
    }
}

fn endpoint_client(endpoint: &str) -> Result<EnrichrClient, ToolError> {
    EnrichrClient::builder()
        .endpoint(endpoint)
        .build()
        .map_err(|error| validation(&error.to_string()))
}

#[tool(
    name = "enrichr_libraries",
    description = "List Enrichr gene-set libraries (KEGG, Reactome, GO, ChEA, MSigDB, \
                   disease and cell-type resources). Filter by a case-insensitive substring \
                   of the library name to discover the exact backgroundType value for \
                   enrichment calls."
)]
pub struct LibrariesInput {
    #[desc = "Optional case-insensitive substring of a library name, e.g. 'KEGG' or 'GO_Biological'."]
    pub query: Option<String>,
    #[desc = "Maximum libraries to display (default 30)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct LibrariesTool {
    client: Arc<EnrichrClient>,
}

impl LibrariesTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for LibrariesTool {
    type Input = LibrariesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let stats = match input.endpoint.as_deref() {
            Some(endpoint) => endpoint_client(endpoint)?
                .dataset_statistics()
                .await
                .map_err(request_error)?,
            None => self
                .client
                .dataset_statistics()
                .await
                .map_err(request_error)?,
        };
        let query = input.query.as_deref().unwrap_or_default();
        let mut matches = stats.filter(query);
        matches.sort_by(|a, b| {
            b.num_terms
                .cmp(&a.num_terms)
                .then(a.library_name.cmp(&b.library_name))
        });
        let total = matches.len();
        let limit = input.limit.unwrap_or(30).max(1);
        Ok(AgentToolResult::success(format::format_libraries(
            &matches, total, limit,
        )))
    }
}

#[tool(
    name = "enrichr_add_list",
    description = "Submit a gene-symbol list to Enrichr and receive a persistent \
                   user_list_id plus a shareable short link. The ID can be reused for \
                   enrich, view, and export calls without resubmitting the genes."
)]
pub struct AddListInput {
    #[desc = "Gene symbols, e.g. TP53, BRCA1, EGFR."]
    pub genes: Vec<String>,
    #[desc = "Optional description stored with the list."]
    pub description: Option<String>,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct AddListTool {
    client: Arc<EnrichrClient>,
}

impl AddListTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for AddListTool {
    type Input = AddListInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.genes.is_empty() {
            return Err(validation("genes must contain at least one symbol"));
        }
        let (added, share_url) = match input.endpoint.as_deref() {
            Some(endpoint) => {
                let client = endpoint_client(endpoint)?;
                let added = client
                    .add_list(&input.genes, input.description.as_deref())
                    .await
                    .map_err(request_error)?;
                let url = client.share_url(added.short_id.as_deref().unwrap_or_default());
                (added, url)
            }
            None => {
                let added = self
                    .client
                    .add_list(&input.genes, input.description.as_deref())
                    .await
                    .map_err(request_error)?;
                let url = self
                    .client
                    .share_url(added.short_id.as_deref().unwrap_or_default());
                (added, url)
            }
        };
        Ok(AgentToolResult::success(format::format_added_list(
            &added, &share_url,
        )))
    }
}

#[tool(
    name = "enrichr_enrich",
    description = "Run Enrichr over-representation enrichment for a gene list against one \
                   gene-set library and return the top terms ranked by significance. \
                   Provide either genes (submitted fresh) or a user_list_id from a prior \
                   enrichr_add_list call."
)]
pub struct EnrichInput {
    #[desc = "Gene symbols to submit. Required unless user_list_id is supplied."]
    pub genes: Option<Vec<String>>,
    #[desc = "Persistent list ID from enrichr_add_list. Required unless genes are supplied."]
    pub user_list_id: Option<u64>,
    #[desc = "Gene-set library name (backgroundType), e.g. KEGG_2021_Human or GO_Biological_Process_2025. Use enrichr_libraries to discover names."]
    pub background_type: String,
    #[desc = "Maximum terms to display (default 15)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct EnrichTool {
    client: Arc<EnrichrClient>,
}

impl EnrichTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

async fn enrich_and_format(
    client: &EnrichrClient,
    genes: Option<Vec<String>>,
    user_list_id: Option<u64>,
    background_type: &str,
    limit: usize,
) -> Result<AgentToolResult, ToolError> {
    let user_list_id = match user_list_id {
        Some(id) => id,
        None => {
            client
                .add_list(genes.unwrap_or_default(), None)
                .await
                .map_err(request_error)?
                .user_list_id
        }
    };
    let result = client
        .enrich(user_list_id, background_type)
        .await
        .map_err(request_error)?;
    Ok(AgentToolResult::success(format::format_enrichment(
        &result, limit,
    )))
}

#[async_trait]
impl ToolFunction for EnrichTool {
    type Input = EnrichInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let has_genes = input.genes.as_ref().is_some_and(|genes| !genes.is_empty());
        let user_list_id = match (has_genes, input.user_list_id) {
            (true, Some(_)) => {
                return Err(validation("provide either genes or user_list_id, not both"));
            }
            (true, None) => None,
            (false, Some(id)) => Some(id),
            (false, None) => {
                return Err(validation("either genes or user_list_id is required"));
            }
        };
        let limit = input.limit.unwrap_or(15).max(1);
        match input.endpoint.as_deref() {
            Some(endpoint) => {
                enrich_and_format(
                    &endpoint_client(endpoint)?,
                    input.genes,
                    user_list_id,
                    &input.background_type,
                    limit,
                )
                .await
            }
            None => {
                enrich_and_format(
                    &self.client,
                    input.genes,
                    user_list_id,
                    &input.background_type,
                    limit,
                )
                .await
            }
        }
    }
}

#[tool(
    name = "enrichr_view_list",
    description = "Inspect a gene list previously submitted to Enrichr: the recognized \
                   gene symbols and the stored description."
)]
pub struct ViewListInput {
    #[desc = "Persistent list ID returned by enrichr_add_list."]
    pub user_list_id: u64,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct ViewListTool {
    client: Arc<EnrichrClient>,
}

impl ViewListTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ViewListTool {
    type Input = ViewListInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let viewed = match input.endpoint.as_deref() {
            Some(endpoint) => endpoint_client(endpoint)?
                .view(input.user_list_id)
                .await
                .map_err(request_error)?,
            None => self
                .client
                .view(input.user_list_id)
                .await
                .map_err(request_error)?,
        };
        Ok(AgentToolResult::success(format::format_viewed_list(
            &viewed,
        )))
    }
}

#[tool(
    name = "enrichr_background_enrich",
    description = "Run Speedrichr background-corrected enrichment: supply the query gene \
                   list and the full background gene universe the experiment could have \
                   observed. Returns top terms with odds ratios computed against the \
                   custom background instead of the whole genome."
)]
pub struct BackgroundEnrichInput {
    #[desc = "Query gene symbols, e.g. the significant hits of a screen."]
    pub genes: Vec<String>,
    #[desc = "Background gene universe: every gene the assay could have detected."]
    pub background_genes: Vec<String>,
    #[desc = "Gene-set library name (backgroundType), e.g. KEGG_2021_Human."]
    pub background_type: String,
    #[desc = "Maximum terms to display (default 15)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct BackgroundEnrichTool {
    client: Arc<EnrichrClient>,
}

impl BackgroundEnrichTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

async fn background_enrich_and_format(
    client: &EnrichrClient,
    genes: Vec<String>,
    background_genes: Vec<String>,
    background_type: &str,
    limit: usize,
) -> Result<AgentToolResult, ToolError> {
    let list = client
        .speedrichr_add_list(genes, None)
        .await
        .map_err(request_error)?;
    let background = client
        .speedrichr_add_background(background_genes)
        .await
        .map_err(request_error)?;
    let result = client
        .speedrichr_background_enrich(
            list.user_list_id,
            &background.background_id,
            background_type,
        )
        .await
        .map_err(request_error)?;
    Ok(AgentToolResult::success(format::format_enrichment(
        &result, limit,
    )))
}

#[async_trait]
impl ToolFunction for BackgroundEnrichTool {
    type Input = BackgroundEnrichInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.genes.is_empty() {
            return Err(validation("genes must contain at least one symbol"));
        }
        if input.background_genes.is_empty() {
            return Err(validation(
                "background_genes must contain at least one symbol",
            ));
        }
        let limit = input.limit.unwrap_or(15).max(1);
        match input.endpoint.as_deref() {
            Some(endpoint) => {
                background_enrich_and_format(
                    &endpoint_client(endpoint)?,
                    input.genes,
                    input.background_genes,
                    &input.background_type,
                    limit,
                )
                .await
            }
            None => {
                background_enrich_and_format(
                    &self.client,
                    input.genes,
                    input.background_genes,
                    &input.background_type,
                    limit,
                )
                .await
            }
        }
    }
}

#[tool(
    name = "enrichr_gene_map",
    description = "Discover which gene-set terms contain a gene across every Enrichr \
                   library — useful for annotating a single gene of interest."
)]
pub struct GeneMapInput {
    #[desc = "Gene symbol, e.g. TP53."]
    pub gene: String,
    #[desc = "Maximum libraries to display (default 20)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct GeneMapTool {
    client: Arc<EnrichrClient>,
}

impl GeneMapTool {
    pub fn new(client: Arc<EnrichrClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for GeneMapTool {
    type Input = GeneMapInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let map = match input.endpoint.as_deref() {
            Some(endpoint) => endpoint_client(endpoint)?
                .genemap(&input.gene)
                .await
                .map_err(request_error)?,
            None => self
                .client
                .genemap(&input.gene)
                .await
                .map_err(request_error)?,
        };
        let limit = input.limit.unwrap_or(20).max(1);
        Ok(AgentToolResult::success(format::format_gene_map(
            &map, limit,
        )))
    }
}

/// Build registrations for every Enrichr agent tool.
pub fn enrichr_registrations(client: Arc<EnrichrClient>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(LibrariesTool::new(client.clone())),
        ToolRegistration::from(AddListTool::new(client.clone())),
        ToolRegistration::from(EnrichTool::new(client.clone())),
        ToolRegistration::from(ViewListTool::new(client.clone())),
        ToolRegistration::from(BackgroundEnrichTool::new(client.clone())),
        ToolRegistration::from(GeneMapTool::new(client)),
    ]
}
