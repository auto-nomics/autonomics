//! Agent tools for identifier resolution, network queries, and previews.

// Tool Input structs marked `#[deprecated]` so the derived tool
// schema advertises `"deprecated": true` (survey T3 dual-track
// guidance); the module-local allow keeps the macro-generated
// impls in this file warning-free.
#![allow(deprecated)]

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult as AgentToolResult, ToolResultBlock};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::request::{
    EnrichmentQuery, ImageFormat, NetworkImageQuery, NetworkQuery, StringIdQuery,
};
use crate::types::PpiEnrichment;
use crate::{StringDbClient, format};

fn request_error(error: crate::StringError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

fn validation(message: &str) -> ToolError {
    ToolError::ValidationFailed {
        message: message.to_owned(),
    }
}

fn network_type(input: Option<&str>) -> Result<crate::NetworkType, ToolError> {
    match input.map(str::trim).unwrap_or("functional") {
        "" | "functional" => Ok(crate::NetworkType::Functional),
        "physical" => Ok(crate::NetworkType::Physical),
        other => Err(validation(&format!(
            "network_type must be 'functional' or 'physical', got '{other}'"
        ))),
    }
}

fn image_format(input: Option<&str>) -> Result<ImageFormat, ToolError> {
    match input.map(str::trim).unwrap_or("png") {
        "" | "png" => Ok(ImageFormat::Png),
        "highres" | "highres_png" | "highres_image" => Ok(ImageFormat::HighResolutionPng),
        "svg" => Ok(ImageFormat::Svg),
        other => Err(validation(&format!(
            "image_format must be 'png', 'highres', or 'svg', got '{other}'"
        ))),
    }
}

#[tool(
    name = "string_resolve_identifiers",
    description = "Resolve protein names, gene symbols, synonyms, or UniProt accessions to \
                   stable STRING identifiers. Prefer resolving identifiers before network or \
                   enrichment calls."
)]
pub struct ResolveIdentifiersInput {
    #[desc = "Protein or gene identifiers to resolve."]
    pub identifiers: Vec<String>,
    #[desc = "Optional NCBI/STRING taxon ID, such as 9606 for human."]
    pub species: Option<String>,
    #[desc = "Maximum mappings to display (default 20)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or version-pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct ResolveIdentifiersTool {
    client: Arc<StringDbClient>,
}

impl ResolveIdentifiersTool {
    pub fn new(client: Arc<StringDbClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ResolveIdentifiersTool {
    type Input = ResolveIdentifiersInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.identifiers.is_empty() {
            return Err(validation("identifiers must contain at least one value"));
        }
        let query = StringIdQuery::new(input.identifiers).species_opt(input.species);
        let rows = if let Some(endpoint) = input.endpoint {
            endpoint_client(endpoint)?
                .get_string_ids(&query)
                .await
                .map_err(request_error)?
        } else {
            self.client
                .get_string_ids(&query)
                .await
                .map_err(request_error)?
        };
        let limit = input.limit.unwrap_or(20).max(1);
        Ok(AgentToolResult::success(format::format_string_ids(
            &rows, limit,
        )))
    }
}

#[tool(
    name = "string_network_interactions",
    description = "Pipeline/dataframe use: prefer the DAG node `source_string_network` (typed table) — this tool stays for interactive lookup. Retrieve STRING protein-protein interactions as a concise Markdown preview. \
                   With one input protein STRING adds a confidence-ranked neighborhood; with two \
                   or more it returns interactions among the supplied proteins."
)]
#[deprecated(note = "prefer the DAG node source_string_network for pipeline use")]
pub struct NetworkInteractionsInput {
    #[desc = "Protein, gene, or STRING identifiers."]
    pub identifiers: Vec<String>,
    #[desc = "Optional NCBI/STRING taxon ID. Required for sets larger than 10 proteins."]
    pub species: Option<String>,
    #[desc = "Optional STRING significance threshold from 0 to 1000; omit to use STRING's default."]
    pub required_score: Option<u16>,
    #[desc = "'functional' (default) or 'physical'."]
    pub network_type: Option<String>,
    #[desc = "Number of interactions to display (default 20)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or version-pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct NetworkInteractionsTool {
    client: Arc<StringDbClient>,
}

impl NetworkInteractionsTool {
    pub fn new(client: Arc<StringDbClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for NetworkInteractionsTool {
    type Input = NetworkInteractionsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let query = network_query(
            input.identifiers,
            input.species,
            input.required_score,
            network_type(input.network_type.as_deref())?,
        )?;
        if query.identifiers.is_empty() {
            return Err(validation("identifiers must contain at least one value"));
        }
        let rows = if let Some(endpoint) = input.endpoint {
            endpoint_client(endpoint)?
                .network(&query)
                .await
                .map_err(request_error)?
        } else {
            self.client.network(&query).await.map_err(request_error)?
        };
        let limit = input.limit.unwrap_or(20).max(1);
        Ok(AgentToolResult::success(format::format_interactions(
            &rows, limit,
        )))
    }
}

#[tool(
    name = "string_functional_enrichment",
    description = "Pipeline/dataframe use: prefer the DAG node `source_string_enrichment` (typed table) — this tool stays for interactive lookup. Run STRING over-representation enrichment for a protein set and return the \
                   most significant functional terms, sorted by FDR."
)]
#[deprecated(note = "prefer the DAG node source_string_enrichment for pipeline use")]
pub struct FunctionalEnrichmentInput {
    #[desc = "Protein, gene, or STRING identifiers comprising the measured set."]
    pub identifiers: Vec<String>,
    #[desc = "Optional NCBI/STRING taxon ID."]
    pub species: Option<String>,
    #[desc = "Optional STRING identifiers defining the experiment background."]
    pub background_string_identifiers: Option<Vec<String>>,
    #[desc = "Maximum terms to display (default 15)."]
    pub limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or version-pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct FunctionalEnrichmentTool {
    client: Arc<StringDbClient>,
}

impl FunctionalEnrichmentTool {
    pub fn new(client: Arc<StringDbClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for FunctionalEnrichmentTool {
    type Input = FunctionalEnrichmentInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.identifiers.is_empty() {
            return Err(validation("identifiers must contain at least one value"));
        }
        let mut query = EnrichmentQuery::new(input.identifiers).species_opt(input.species);
        if let Some(background) = input.background_string_identifiers {
            query = query.background(background);
        }
        let rows = if let Some(endpoint) = input.endpoint {
            endpoint_client(endpoint)?
                .enrichment(&query)
                .await
                .map_err(request_error)?
        } else {
            self.client
                .enrichment(&query)
                .await
                .map_err(request_error)?
        };
        let limit = input.limit.unwrap_or(15).max(1);
        Ok(AgentToolResult::success(format::format_enrichment(
            &rows, limit,
        )))
    }
}

#[tool(
    name = "string_network_summary",
    description = "Build a compact STRING biological summary: top interactions, PPI enrichment, \
                   and most significant functional enrichment terms for a protein set."
)]
pub struct NetworkSummaryInput {
    #[desc = "Protein, gene, or STRING identifiers to summarize."]
    pub identifiers: Vec<String>,
    #[desc = "Optional NCBI/STRING taxon ID."]
    pub species: Option<String>,
    #[desc = "Optional STRING significance threshold from 0 to 1000; omit to use STRING's default."]
    pub required_score: Option<u16>,
    #[desc = "'functional' (default) or 'physical'."]
    pub network_type: Option<String>,
    #[desc = "Maximum enrichment terms included in the summary (default 8)."]
    pub enrichment_limit: Option<usize>,
    #[desc = "Optional endpoint override for tests or version-pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct NetworkSummaryTool {
    client: Arc<StringDbClient>,
}

impl NetworkSummaryTool {
    pub fn new(client: Arc<StringDbClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for NetworkSummaryTool {
    type Input = NetworkSummaryInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.identifiers.len() < 2 {
            return Err(validation(
                "string_network_summary requires at least two identifiers so enrichment is \
                 not computed on an automatically expanded one-protein neighborhood",
            ));
        }
        let network_query = network_query(
            input.identifiers.clone(),
            input.species.clone(),
            input.required_score,
            network_type(input.network_type.as_deref())?,
        )?;
        let enrichment_query = EnrichmentQuery::new(input.identifiers).species_opt(input.species);

        if let Some(endpoint) = input.endpoint {
            let client = endpoint_client(endpoint)?;
            summarize(
                &client,
                &network_query,
                &enrichment_query,
                input.enrichment_limit,
            )
            .await
        } else {
            summarize(
                &self.client,
                &network_query,
                &enrichment_query,
                input.enrichment_limit,
            )
            .await
        }
    }
}

async fn summarize(
    client: &StringDbClient,
    network_query: &NetworkQuery,
    enrichment_query: &EnrichmentQuery,
    enrichment_limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let interactions = client.network(network_query).await.map_err(request_error)?;
    let ppi = first(
        client
            .ppi_enrichment(enrichment_query)
            .await
            .map_err(request_error)?,
    );
    let mut enrichment = client
        .enrichment(enrichment_query)
        .await
        .map_err(request_error)?;
    enrichment.sort_by(|a, b| a.fdr.total_cmp(&b.fdr));
    let limit = enrichment_limit.unwrap_or(8).max(1);

    let mut out = format!(
        "## STRING network summary\n\n- Input proteins: {}\n- Retrieved interactions: {}\n",
        network_query.identifiers.len(),
        interactions.len(),
    );
    if let Some(ppi) = ppi {
        out.push_str(&format!(
            "- PPI enrichment: {} observed / {:.3} expected edges, p = {}\n",
            ppi.number_of_edges,
            ppi.expected_number_of_edges,
            compact_number(ppi.p_value),
        ));
    }
    if let Some(row) = interactions.first() {
        out.push_str(&format!(
            "- Strongest edge: {} -- {} (combined score {:.3})\n",
            row.preferred_name_a.as_deref().unwrap_or(&row.string_id_a),
            row.preferred_name_b.as_deref().unwrap_or(&row.string_id_b),
            row.score,
        ));
    }
    if enrichment.is_empty() {
        out.push_str("\nNo functional enrichment terms were returned.\n");
    } else {
        out.push_str("\n**Top functional terms**\n\n");
        out.push_str(&format::format_enrichment(&enrichment, limit));
    }
    Ok(AgentToolResult::success(out))
}

#[tool(
    name = "string_network_image",
    description = "Render a STRING network image for visual preview. Returns a text caption plus \
                   a PNG or SVG image block."
)]
pub struct NetworkImageInput {
    #[desc = "Protein, gene, or STRING identifiers."]
    pub identifiers: Vec<String>,
    #[desc = "Optional NCBI/STRING taxon ID. Required for sets larger than 10 proteins."]
    pub species: Option<String>,
    #[desc = "Optional STRING significance threshold from 0 to 1000; omit to use STRING's default."]
    pub required_score: Option<u16>,
    #[desc = "'functional' (default) or 'physical'."]
    pub network_type: Option<String>,
    #[desc = "Optional number of confidence-ranked color nodes to add."]
    pub add_color_nodes: Option<u16>,
    #[desc = "Optional number of confidence-ranked white neighborhood nodes to add."]
    pub add_white_nodes: Option<u16>,
    #[desc = "'png' (default), 'highres', or 'svg'."]
    pub image_format: Option<String>,
    #[desc = "Optional endpoint override for tests or version-pinned deployments."]
    pub endpoint: Option<String>,
}

pub struct NetworkImageTool {
    client: Arc<StringDbClient>,
}

impl NetworkImageTool {
    pub fn new(client: Arc<StringDbClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for NetworkImageTool {
    type Input = NetworkImageInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.identifiers.is_empty() {
            return Err(validation("identifiers must contain at least one value"));
        }
        let network = network_query(
            input.identifiers,
            input.species,
            input.required_score,
            network_type(input.network_type.as_deref())?,
        )?;
        let mut query = NetworkImageQuery::new(network);
        if let Some(count) = input.add_color_nodes {
            query = query.add_color_nodes(count);
        }
        if let Some(count) = input.add_white_nodes {
            query = query.add_white_nodes(count);
        }
        let requested_format = image_format(input.image_format.as_deref())?;
        let image = if let Some(endpoint) = input.endpoint {
            endpoint_client(endpoint)?
                .network_image(&query, requested_format)
                .await
                .map_err(request_error)?
        } else {
            self.client
                .network_image(&query, requested_format)
                .await
                .map_err(request_error)?
        };

        Ok(AgentToolResult::with_blocks(vec![
            ToolResultBlock::text(format!(
                "STRING network preview ({} bytes, {})",
                image.bytes.len(),
                image.media_type,
            )),
            ToolResultBlock::image_base64(image.media_type, STANDARD.encode(image.bytes)),
        ]))
    }
}

/// Build registrations for every STRING agent tool.
pub fn string_registrations(client: Arc<StringDbClient>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(ResolveIdentifiersTool::new(client.clone())),
        ToolRegistration::from(NetworkInteractionsTool::new(client.clone())),
        ToolRegistration::from(FunctionalEnrichmentTool::new(client.clone())),
        ToolRegistration::from(NetworkSummaryTool::new(client.clone())),
        ToolRegistration::from(NetworkImageTool::new(client)),
    ]
}

fn network_query(
    identifiers: Vec<String>,
    species: Option<String>,
    required_score: Option<u16>,
    network_type: crate::NetworkType,
) -> Result<NetworkQuery, ToolError> {
    let mut query = NetworkQuery::new(identifiers).species_opt(species);
    if let Some(score) = required_score {
        query = query.required_score(score);
    }
    Ok(query.network_type(network_type))
}

fn endpoint_client(endpoint: String) -> Result<StringDbClient, ToolError> {
    StringDbClient::builder()
        .caller_identity("autonomics-string-tools")
        .endpoint(endpoint)
        .build()
        .map_err(|error| validation(&error.to_string()))
}

fn first(mut rows: Vec<PpiEnrichment>) -> Option<PpiEnrichment> {
    if rows.len() > 1 {
        Some(rows.swap_remove(0))
    } else {
        rows.pop()
    }
}

fn compact_number(value: f64) -> String {
    if value == 0.0 || !(0.0001..=9999.5).contains(&value.abs()) {
        format!("{value:.3e}")
    } else {
        format!("{value:.3}")
    }
}
