//! DAG source nodes that emit Enrichr results as DataFrames.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodePlugin, NodeRegistry};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::{EnrichrClient, request::validate_library_name};

/// Register every Enrichr DAG source node as one plugin.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "enrichr"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(EnrichrEnrichmentNodeFactory));
        registry.register(Box::new(EnrichrLibrariesNodeFactory));
        registry.register(Box::new(EnrichrViewListNodeFactory));
        registry.register(Box::new(EnrichrGeneMapNodeFactory));
        registry.register(Box::new(EnrichrBackgroundEnrichmentNodeFactory));
    }

    fn fixture_spec(&self, kind: &str) -> Option<serde_json::Value> {
        match kind {
            "source_enrichr_enrich" => Some(serde_json::json!({
                "genes": ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
                "background_type": "KEGG_2021_Human",
            })),
            "source_enrichr_libraries" => Some(serde_json::json!({
                "query": "KEGG",
            })),
            "source_enrichr_view_list" | "source_enrichr_genemap" => None,
            "source_enrichr_background_enrich" => Some(serde_json::json!({
                "genes": ["TP53", "BRCA1", "EGFR", "MYC", "PTEN"],
                "background_genes": [
                    "TP53", "BRCA1", "EGFR", "MYC", "PTEN", "AKT1", "KRAS",
                    "CDK2", "RB1", "MDM2"
                ],
                "background_type": "KEGG_2021_Human",
            })),
            _ => None,
        }
    }
}

fn source_error(error: crate::EnrichrError, operation: &str) -> DagError {
    DagError::Schedule(format!("Enrichr {operation} failed: {error}"))
}

fn client(endpoint: &Option<String>) -> Result<EnrichrClient, DagError> {
    let mut builder = EnrichrClient::builder();
    if let Some(endpoint) = endpoint {
        builder = builder.endpoint(endpoint);
    }
    builder
        .build()
        .map_err(|error| DagError::Schedule(format!("invalid Enrichr client: {error}")))
}

fn speedrichr_client(endpoint: &Option<String>) -> Result<EnrichrClient, DagError> {
    let mut builder = EnrichrClient::builder();
    if let Some(endpoint) = endpoint {
        builder = builder.speedrichr_endpoint(endpoint);
    }
    builder
        .build()
        .map_err(|error| DagError::Schedule(format!("invalid Speedrichr client: {error}")))
}

fn str_array(values: Vec<Option<String>>) -> Arc<dyn Array> {
    let values: Vec<Option<&str>> = values.iter().map(Option::as_deref).collect();
    Arc::new(StringArray::from(values))
}

fn f64_array(values: Vec<Option<f64>>) -> Arc<dyn Array> {
    Arc::new(Float64Array::from(values))
}

fn u64_array(values: Vec<Option<u64>>) -> Arc<dyn Array> {
    Arc::new(UInt64Array::from(values))
}

fn output(df: datafusion::dataframe::DataFrame) -> Result<PortOutputs, DagError> {
    let mut outputs = PortOutputs::new();
    outputs.insert(0, df);
    Ok(outputs)
}

async fn read_batch(
    ctx: &NodeCtx,
    batch: RecordBatch,
    operation: &str,
) -> Result<datafusion::dataframe::DataFrame, DagError> {
    ctx.session()
        .read_batch(batch)
        .map_err(|error| DagError::Schedule(format!("failed to read Enrichr {operation}: {error}")))
}

// ===========================================================================
// source_enrichr_enrich
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct EnrichrEnrichmentSpec {
    /// Gene symbols to submit; ignored when `user_list_id` is set.
    pub genes: Vec<String>,
    /// Persistent list ID from a prior `addList` submission.
    pub user_list_id: Option<u64>,
    /// Gene-set library name (`backgroundType`).
    pub background_type: String,
    /// Optional endpoint override for pinned deployments.
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct EnrichrEnrichmentNode {
    meta: NodePorts,
    spec: EnrichrEnrichmentSpec,
}

pub struct EnrichrEnrichmentNodeFactory;

impl NodeFactory for EnrichrEnrichmentNodeFactory {
    fn kind(&self) -> &'static str {
        "source_enrichr_enrich"
    }

    fn desc(&self) -> &'static str {
        "Run Enrichr over-representation enrichment for a gene list."
    }

    fn doc(&self) -> &'static str {
        "Submit genes (or reuse a user_list_id) and enrich against one library. Output \
         columns: library, rank, term, overlap_count, overlapping_genes, p_value, \
         adjusted_p_value, z_score, combined_score, old_p_value, old_adjusted_p_value. \
         Use source_enrichr_libraries to discover background_type names."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichrEnrichmentSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(EnrichrEnrichmentNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for EnrichrEnrichmentNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_enrichr_enrich"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client(&self.spec.endpoint)?;
        let user_list_id = match self.spec.user_list_id {
            Some(id) => id,
            None => {
                client
                    .add_list(self.spec.genes.clone(), Some("source_enrichr_enrich"))
                    .await
                    .map_err(|error| source_error(error, "list submission"))?
                    .user_list_id
            }
        };
        let result = client
            .enrich(user_list_id, &self.spec.background_type)
            .await
            .map_err(|error| source_error(error, "enrichment"))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("library", DataType::Utf8, false),
                Field::new("rank", DataType::UInt64, false),
                Field::new("term", DataType::Utf8, false),
                Field::new("overlap_count", DataType::UInt64, false),
                Field::new("overlapping_genes", DataType::Utf8, true),
                Field::new("p_value", DataType::Float64, false),
                Field::new("adjusted_p_value", DataType::Float64, false),
                Field::new("z_score", DataType::Float64, true),
                Field::new("combined_score", DataType::Float64, false),
                Field::new("old_p_value", DataType::Float64, true),
                Field::new("old_adjusted_p_value", DataType::Float64, true),
            ])),
            vec![
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|_| Some(result.library.clone()))
                        .collect(),
                ),
                u64_array(result.terms.iter().map(|t| Some(t.rank)).collect()),
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.term.clone()))
                        .collect(),
                ),
                u64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.overlap_count() as u64))
                        .collect(),
                ),
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|term| {
                            (!term.overlapping_genes.is_empty())
                                .then(|| term.overlapping_genes.join(","))
                        })
                        .collect(),
                ),
                f64_array(result.terms.iter().map(|t| Some(t.p_value)).collect()),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|t| Some(t.adjusted_p_value))
                        .collect(),
                ),
                f64_array(result.terms.iter().map(|t| Some(t.z_score)).collect()),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|t| Some(t.combined_score))
                        .collect(),
                ),
                f64_array(result.terms.iter().map(|t| Some(t.old_p_value)).collect()),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|t| Some(t.old_adjusted_p_value))
                        .collect(),
                ),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build Enrichr enrichment batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "enrichment").await?;
        output(df)
    }
}

// ===========================================================================
// source_enrichr_libraries
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct EnrichrLibrariesSpec {
    /// Optional case-insensitive substring filter on the library name.
    pub query: Option<String>,
    /// Optional endpoint override for pinned deployments.
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct EnrichrLibrariesNode {
    meta: NodePorts,
    spec: EnrichrLibrariesSpec,
}

pub struct EnrichrLibrariesNodeFactory;

impl NodeFactory for EnrichrLibrariesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_enrichr_libraries"
    }

    fn desc(&self) -> &'static str {
        "List Enrichr gene-set libraries and their statistics."
    }

    fn doc(&self) -> &'static str {
        "Catalog of every available backgroundType. Output columns: library_name, \
         num_terms, gene_coverage, genes_per_term, link, category_id. Filter with the \
         optional query substring."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichrLibrariesSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(EnrichrLibrariesNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for EnrichrLibrariesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_enrichr_libraries"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client(&self.spec.endpoint)?;
        let stats = client
            .dataset_statistics()
            .await
            .map_err(|error| source_error(error, "dataset statistics"))?;
        let mut rows = stats.filter(self.spec.query.as_deref().unwrap_or_default());
        rows.sort_by(|a, b| a.library_name.cmp(&b.library_name));

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("library_name", DataType::Utf8, false),
                Field::new("num_terms", DataType::UInt64, true),
                Field::new("gene_coverage", DataType::UInt64, true),
                Field::new("genes_per_term", DataType::Float64, true),
                Field::new("link", DataType::Utf8, true),
                Field::new("category_id", DataType::UInt64, true),
            ])),
            vec![
                str_array(
                    rows.iter()
                        .map(|row| Some(row.library_name.clone()))
                        .collect(),
                ),
                u64_array(rows.iter().map(|row| row.num_terms).collect()),
                u64_array(rows.iter().map(|row| row.gene_coverage).collect()),
                f64_array(rows.iter().map(|row| row.genes_per_term).collect()),
                str_array(rows.iter().map(|row| row.link.clone()).collect()),
                u64_array(rows.iter().map(|row| row.category_id).collect()),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build Enrichr libraries batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "libraries").await?;
        output(df)
    }
}

// ===========================================================================
// source_enrichr_view_list
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct EnrichrViewListSpec {
    /// Persistent list ID returned by `addList`.
    pub user_list_id: u64,
    /// Optional endpoint override for pinned deployments.
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct EnrichrViewListNode {
    meta: NodePorts,
    spec: EnrichrViewListSpec,
}

pub struct EnrichrViewListNodeFactory;

impl NodeFactory for EnrichrViewListNodeFactory {
    fn kind(&self) -> &'static str {
        "source_enrichr_view_list"
    }

    fn desc(&self) -> &'static str {
        "Read back a gene list previously submitted to Enrichr."
    }

    fn doc(&self) -> &'static str {
        "One row per recognized gene. Output columns: user_list_id, gene, description."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichrViewListSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(EnrichrViewListNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for EnrichrViewListNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_enrichr_view_list"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client(&self.spec.endpoint)?;
        let viewed = client
            .view(self.spec.user_list_id)
            .await
            .map_err(|error| source_error(error, "view list"))?;
        let description = viewed.description.clone();

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("user_list_id", DataType::UInt64, false),
                Field::new("gene", DataType::Utf8, false),
                Field::new("description", DataType::Utf8, true),
            ])),
            vec![
                u64_array(
                    viewed
                        .genes
                        .iter()
                        .map(|_| Some(self.spec.user_list_id))
                        .collect(),
                ),
                str_array(viewed.genes.iter().map(|gene| Some(gene.clone())).collect()),
                str_array(viewed.genes.iter().map(|_| description.clone()).collect()),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build Enrichr view batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "view list").await?;
        output(df)
    }
}

// ===========================================================================
// source_enrichr_genemap
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct EnrichrGeneMapSpec {
    /// Gene symbol to annotate.
    pub gene: String,
    /// Optional endpoint override for pinned deployments.
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct EnrichrGeneMapNode {
    meta: NodePorts,
    spec: EnrichrGeneMapSpec,
}

pub struct EnrichrGeneMapNodeFactory;

impl NodeFactory for EnrichrGeneMapNodeFactory {
    fn kind(&self) -> &'static str {
        "source_enrichr_genemap"
    }

    fn desc(&self) -> &'static str {
        "List the gene-set terms containing one gene, across every library."
    }

    fn doc(&self) -> &'static str {
        "Flattened library/term rows for one gene. Output columns: gene, library, \
         term_index, term."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichrGeneMapSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(EnrichrGeneMapNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for EnrichrGeneMapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_enrichr_genemap"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client(&self.spec.endpoint)?;
        let map = client
            .genemap(&self.spec.gene)
            .await
            .map_err(|error| source_error(error, "genemap"))?;
        let libraries: BTreeMap<String, Vec<String>> = map.libraries;
        let row_count: usize = libraries.values().map(Vec::len).sum();

        let mut gene_column = Vec::with_capacity(row_count);
        let mut library_column = Vec::with_capacity(row_count);
        let mut index_column = Vec::with_capacity(row_count);
        let mut term_column = Vec::with_capacity(row_count);
        for (library, terms) in &libraries {
            for (index, term) in terms.iter().enumerate() {
                gene_column.push(Some(self.spec.gene.clone()));
                library_column.push(Some(library.clone()));
                index_column.push(Some(index as u64));
                term_column.push(Some(term.clone()));
            }
        }

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("gene", DataType::Utf8, false),
                Field::new("library", DataType::Utf8, false),
                Field::new("term_index", DataType::UInt64, false),
                Field::new("term", DataType::Utf8, false),
            ])),
            vec![
                str_array(gene_column),
                str_array(library_column),
                u64_array(index_column),
                str_array(term_column),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build Enrichr genemap batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "genemap").await?;
        output(df)
    }
}

// ===========================================================================
// source_enrichr_background_enrich
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct EnrichrBackgroundEnrichmentSpec {
    /// Query gene symbols, such as the significant hits from a screen.
    pub genes: Vec<String>,
    /// Full background gene universe the assay could have observed.
    pub background_genes: Vec<String>,
    /// Gene-set library name (`backgroundType`).
    pub background_type: String,
    /// Optional Speedrichr endpoint override for pinned deployments.
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct EnrichrBackgroundEnrichmentNode {
    meta: NodePorts,
    spec: EnrichrBackgroundEnrichmentSpec,
}

pub struct EnrichrBackgroundEnrichmentNodeFactory;

impl NodeFactory for EnrichrBackgroundEnrichmentNodeFactory {
    fn kind(&self) -> &'static str {
        "source_enrichr_background_enrich"
    }

    fn desc(&self) -> &'static str {
        "Run Speedrichr enrichment against a custom background universe."
    }

    fn doc(&self) -> &'static str {
        "Submit a query list and background universe, then run background-corrected \
         enrichment. Output columns: library, rank, term, overlap_count, \
         overlapping_genes, p_value, adjusted_p_value, z_score (odds ratio for \
         Speedrichr), combined_score, \
         old_p_value, old_adjusted_p_value. Use source_enrichr_libraries to discover \
         background_type names."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichrBackgroundEnrichmentSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(EnrichrBackgroundEnrichmentNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for EnrichrBackgroundEnrichmentNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_enrichr_background_enrich"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.genes.is_empty() {
            return Err(DagError::Schedule(
                "source_enrichr_background_enrich: genes must contain at least one symbol"
                    .to_owned(),
            ));
        }
        if self.spec.background_genes.is_empty() {
            return Err(DagError::Schedule(
                "source_enrichr_background_enrich: background_genes must contain at least one symbol"
                    .to_owned(),
            ));
        }
        validate_library_name(&self.spec.background_type)
            .map_err(|error| source_error(error, "background_type validation"))?;

        let client = speedrichr_client(&self.spec.endpoint)?;
        let list = client
            .speedrichr_add_list(
                self.spec.genes.clone(),
                Some("source_enrichr_background_enrich"),
            )
            .await
            .map_err(|error| source_error(error, "background query-list submission"))?;
        let background = client
            .speedrichr_add_background(self.spec.background_genes.clone())
            .await
            .map_err(|error| source_error(error, "background submission"))?;
        let result = client
            .speedrichr_background_enrich(
                list.user_list_id,
                &background.background_id,
                &self.spec.background_type,
            )
            .await
            .map_err(|error| source_error(error, "background enrichment"))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("library", DataType::Utf8, false),
                Field::new("rank", DataType::UInt64, false),
                Field::new("term", DataType::Utf8, false),
                Field::new("overlap_count", DataType::UInt64, false),
                Field::new("overlapping_genes", DataType::Utf8, true),
                Field::new("p_value", DataType::Float64, false),
                Field::new("adjusted_p_value", DataType::Float64, false),
                Field::new("z_score", DataType::Float64, true),
                Field::new("combined_score", DataType::Float64, false),
                Field::new("old_p_value", DataType::Float64, true),
                Field::new("old_adjusted_p_value", DataType::Float64, true),
            ])),
            vec![
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|_| Some(result.library.clone()))
                        .collect(),
                ),
                u64_array(result.terms.iter().map(|term| Some(term.rank)).collect()),
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.term.clone()))
                        .collect(),
                ),
                u64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.overlap_count() as u64))
                        .collect(),
                ),
                str_array(
                    result
                        .terms
                        .iter()
                        .map(|term| {
                            (!term.overlapping_genes.is_empty())
                                .then(|| term.overlapping_genes.join(","))
                        })
                        .collect(),
                ),
                f64_array(result.terms.iter().map(|term| Some(term.p_value)).collect()),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.adjusted_p_value))
                        .collect(),
                ),
                f64_array(result.terms.iter().map(|term| Some(term.z_score)).collect()),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.combined_score))
                        .collect(),
                ),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.old_p_value))
                        .collect(),
                ),
                f64_array(
                    result
                        .terms
                        .iter()
                        .map(|term| Some(term.old_adjusted_p_value))
                        .collect(),
                ),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!(
                "failed to build Speedrichr enrichment batch: {error}"
            ))
        })?;
        let df = read_batch(ctx, batch, "background enrichment").await?;
        output(df)
    }
}
