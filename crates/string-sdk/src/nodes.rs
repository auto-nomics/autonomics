//! DAG source nodes that emit STRING results as DataFrames.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodePlugin, NodeRegistry};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::request::{EnrichmentQuery, NetworkQuery, StringIdQuery};
use crate::{NetworkType, StringDbClient};

/// Register every STRING DAG source node as one plugin.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "string"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(StringIdMapNodeFactory));
        registry.register(Box::new(StringNetworkNodeFactory));
        registry.register(Box::new(StringEnrichmentNodeFactory));
        registry.register(Box::new(StringPpiEnrichmentNodeFactory));
    }

    fn fixture_spec(&self, kind: &str) -> Option<serde_json::Value> {
        match kind {
            "source_string_id_map"
            | "source_string_network"
            | "source_string_enrichment"
            | "source_string_ppi_enrichment" => Some(serde_json::json!({
                "identifiers": ["TP53", "CDK2"],
                "species": "9606",
            })),
            _ => None,
        }
    }
}

fn source_error(error: crate::StringError, operation: &str) -> DagError {
    DagError::Schedule(format!("STRING {operation} failed: {error}"))
}

fn client(
    endpoint: &Option<String>,
    caller_identity: &Option<String>,
) -> Result<StringDbClient, DagError> {
    let mut builder = StringDbClient::builder().caller_identity(
        caller_identity
            .clone()
            .unwrap_or_else(|| "autonomics-string-dag".to_owned()),
    );
    if let Some(endpoint) = endpoint {
        builder = builder.endpoint(endpoint);
    }
    builder
        .build()
        .map_err(|error| DagError::Schedule(format!("invalid STRING client: {error}")))
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

fn optional_join(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(","))
}

fn parse_network_type(value: Option<&str>) -> Result<NetworkType, DagError> {
    match value.map(str::trim).unwrap_or("functional") {
        "" | "functional" => Ok(NetworkType::Functional),
        "physical" => Ok(NetworkType::Physical),
        other => Err(DagError::Schedule(format!(
            "source_string_network: unknown network_type '{other}'"
        ))),
    }
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
        .map_err(|error| DagError::Schedule(format!("failed to read STRING {operation}: {error}")))
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct StringIdMapSpec {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub endpoint: Option<String>,
    pub caller_identity: Option<String>,
}

#[derive(Clone)]
pub struct StringIdMapNode {
    meta: NodePorts,
    spec: StringIdMapSpec,
}

pub struct StringIdMapNodeFactory;

impl NodeFactory for StringIdMapNodeFactory {
    fn kind(&self) -> &'static str {
        "source_string_id_map"
    }

    fn desc(&self) -> &'static str {
        "Map protein or gene identifiers to stable STRING identifiers."
    }

    fn doc(&self) -> &'static str {
        "Resolve common names, synonyms, or UniProt accessions to STRING IDs. Output columns: \
         query_index, query_item, string_id, preferred_name, taxon_id, taxon_name, annotation."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StringIdMapSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(StringIdMapNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for StringIdMapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_string_id_map"
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
        let client = client(&self.spec.endpoint, &self.spec.caller_identity)?;
        let rows = client
            .get_string_ids(
                &StringIdQuery::new(self.spec.identifiers.clone())
                    .species_opt(self.spec.species.clone()),
            )
            .await
            .map_err(|error| source_error(error, "identifier mapping"))?;
        let count = rows.len();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("query_index", DataType::UInt64, false),
                Field::new("query_item", DataType::Utf8, true),
                Field::new("string_id", DataType::Utf8, false),
                Field::new("preferred_name", DataType::Utf8, true),
                Field::new("taxon_id", DataType::UInt64, true),
                Field::new("taxon_name", DataType::Utf8, true),
                Field::new("annotation", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(
                    rows.iter()
                        .map(|row| Some(u64::from(row.query_index)))
                        .collect::<Vec<_>>(),
                )),
                str_array(rows.iter().map(|row| row.query_item.clone()).collect()),
                str_array(rows.iter().map(|row| Some(row.string_id.clone())).collect()),
                str_array(rows.iter().map(|row| row.preferred_name.clone()).collect()),
                u64_array(rows.iter().map(|row| Some(row.ncbi_taxon_id)).collect()),
                str_array(rows.iter().map(|row| row.taxon_name.clone()).collect()),
                str_array(rows.iter().map(|row| row.annotation.clone()).collect()),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build STRING mapping batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "identifier mapping").await?;
        let _ = count;
        output(df)
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct StringNetworkSpec {
    pub identifiers: Vec<String>,
    pub network_term_id: Option<String>,
    pub species: Option<String>,
    pub required_score: Option<u16>,
    pub network_type: Option<String>,
    pub endpoint: Option<String>,
    pub caller_identity: Option<String>,
}

#[derive(Clone)]
pub struct StringNetworkNode {
    meta: NodePorts,
    spec: StringNetworkSpec,
}

pub struct StringNetworkNodeFactory;

impl NodeFactory for StringNetworkNodeFactory {
    fn kind(&self) -> &'static str {
        "source_string_network"
    }

    fn desc(&self) -> &'static str {
        "Fetch STRING protein interactions as a scored edge table."
    }

    fn doc(&self) -> &'static str {
        "Retrieve functional or physical STRING interactions. Output columns: string_id_a, \
         string_id_b, preferred_name_a, preferred_name_b, taxon_id, score, neighborhood_score, \
         fusion_score, cooccurrence_score, coexpression_score, experimental_score, database_score, \
         textmining_score."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StringNetworkSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(StringNetworkNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for StringNetworkNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_string_network"
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
        let has_identifiers = !self.spec.identifiers.is_empty();
        let has_term_id = self.spec.network_term_id.is_some();
        if has_identifiers == has_term_id {
            return Err(DagError::Schedule(
                "source_string_network: provide exactly one of identifiers or network_term_id"
                    .to_owned(),
            ));
        }
        let mut query = if has_term_id {
            NetworkQuery::from_term(self.spec.network_term_id.clone().unwrap_or_default())
        } else {
            NetworkQuery::new(self.spec.identifiers.clone())
        };
        query = query
            .species_opt(self.spec.species.clone())
            .network_type(parse_network_type(self.spec.network_type.as_deref())?);
        if let Some(score) = self.spec.required_score {
            query = query.required_score(score);
        }
        let client = client(&self.spec.endpoint, &self.spec.caller_identity)?;
        let rows = client
            .network(&query)
            .await
            .map_err(|error| source_error(error, "network query"))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("string_id_a", DataType::Utf8, false),
                Field::new("string_id_b", DataType::Utf8, false),
                Field::new("preferred_name_a", DataType::Utf8, true),
                Field::new("preferred_name_b", DataType::Utf8, true),
                Field::new("taxon_id", DataType::UInt64, true),
                Field::new("score", DataType::Float64, true),
                Field::new("neighborhood_score", DataType::Float64, true),
                Field::new("fusion_score", DataType::Float64, true),
                Field::new("cooccurrence_score", DataType::Float64, true),
                Field::new("coexpression_score", DataType::Float64, true),
                Field::new("experimental_score", DataType::Float64, true),
                Field::new("database_score", DataType::Float64, true),
                Field::new("textmining_score", DataType::Float64, true),
            ])),
            vec![
                str_array(
                    rows.iter()
                        .map(|row| Some(row.string_id_a.clone()))
                        .collect(),
                ),
                str_array(
                    rows.iter()
                        .map(|row| Some(row.string_id_b.clone()))
                        .collect(),
                ),
                str_array(
                    rows.iter()
                        .map(|row| row.preferred_name_a.clone())
                        .collect(),
                ),
                str_array(
                    rows.iter()
                        .map(|row| row.preferred_name_b.clone())
                        .collect(),
                ),
                u64_array(rows.iter().map(|row| row.ncbi_taxon_id.as_u64()).collect()),
                f64_array(rows.iter().map(|row| Some(row.score)).collect()),
                f64_array(rows.iter().map(|row| Some(row.nscore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.fscore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.pscore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.ascore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.escore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.dscore)).collect()),
                f64_array(rows.iter().map(|row| Some(row.tscore)).collect()),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build STRING network batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "network").await?;
        output(df)
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct StringEnrichmentSpec {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub background_string_identifiers: Option<Vec<String>>,
    pub endpoint: Option<String>,
    pub caller_identity: Option<String>,
}

#[derive(Clone)]
pub struct StringEnrichmentNode {
    meta: NodePorts,
    spec: StringEnrichmentSpec,
}

pub struct StringEnrichmentNodeFactory;

impl NodeFactory for StringEnrichmentNodeFactory {
    fn kind(&self) -> &'static str {
        "source_string_enrichment"
    }

    fn desc(&self) -> &'static str {
        "Run STRING functional enrichment and emit term statistics."
    }

    fn doc(&self) -> &'static str {
        "Over-representation enrichment for a protein set. Output columns: category, term, \
         genes, background_genes, taxon_id, p_value, fdr, description, input_genes, \
         preferred_names."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StringEnrichmentSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(StringEnrichmentNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for StringEnrichmentNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_string_enrichment"
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
        let mut query = EnrichmentQuery::new(self.spec.identifiers.clone())
            .species_opt(self.spec.species.clone());
        if let Some(background) = &self.spec.background_string_identifiers {
            query = query.background(background.clone());
        }
        let client = client(&self.spec.endpoint, &self.spec.caller_identity)?;
        let rows = client
            .enrichment(&query)
            .await
            .map_err(|error| source_error(error, "enrichment"))?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("category", DataType::Utf8, false),
                Field::new("term", DataType::Utf8, false),
                Field::new("genes", DataType::UInt64, false),
                Field::new("background_genes", DataType::UInt64, false),
                Field::new("taxon_id", DataType::UInt64, true),
                Field::new("p_value", DataType::Float64, false),
                Field::new("fdr", DataType::Float64, false),
                Field::new("description", DataType::Utf8, true),
                Field::new("input_genes", DataType::Utf8, true),
                Field::new("preferred_names", DataType::Utf8, true),
            ])),
            vec![
                str_array(rows.iter().map(|row| Some(row.category.clone())).collect()),
                str_array(rows.iter().map(|row| Some(row.term.clone())).collect()),
                u64_array(rows.iter().map(|row| Some(row.number_of_genes)).collect()),
                u64_array(
                    rows.iter()
                        .map(|row| Some(row.number_of_genes_in_background))
                        .collect(),
                ),
                u64_array(rows.iter().map(|row| row.ncbi_taxon_id.as_u64()).collect()),
                f64_array(rows.iter().map(|row| Some(row.p_value)).collect()),
                f64_array(rows.iter().map(|row| Some(row.fdr)).collect()),
                str_array(rows.iter().map(|row| row.description.clone()).collect()),
                str_array(
                    rows.iter()
                        .map(|row| optional_join(&row.input_genes))
                        .collect(),
                ),
                str_array(
                    rows.iter()
                        .map(|row| optional_join(&row.preferred_names))
                        .collect(),
                ),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build STRING enrichment batch: {error}"))
        })?;
        let df = read_batch(ctx, batch, "enrichment").await?;
        output(df)
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct StringPpiEnrichmentSpec {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub background_string_identifiers: Option<Vec<String>>,
    pub endpoint: Option<String>,
    pub caller_identity: Option<String>,
}

#[derive(Clone)]
pub struct StringPpiEnrichmentNode {
    meta: NodePorts,
    spec: StringPpiEnrichmentSpec,
}

pub struct StringPpiEnrichmentNodeFactory;

impl NodeFactory for StringPpiEnrichmentNodeFactory {
    fn kind(&self) -> &'static str {
        "source_string_ppi_enrichment"
    }

    fn desc(&self) -> &'static str {
        "Test whether a protein set has more STRING interactions than expected."
    }

    fn doc(&self) -> &'static str {
        "Network-level interaction enrichment. Output columns: nodes, edges, average_node_degree, \
         local_clustering_coefficient, expected_edges, p_value."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StringPpiEnrichmentSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(StringPpiEnrichmentNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

#[async_trait]
impl DagNode for StringPpiEnrichmentNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_string_ppi_enrichment"
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
        let mut query = EnrichmentQuery::new(self.spec.identifiers.clone())
            .species_opt(self.spec.species.clone());
        if let Some(background) = &self.spec.background_string_identifiers {
            query = query.background(background.clone());
        }
        let client = client(&self.spec.endpoint, &self.spec.caller_identity)?;
        let rows = client
            .ppi_enrichment(&query)
            .await
            .map_err(|error| source_error(error, "PPI enrichment"))?;
        let row = rows.first().ok_or_else(|| {
            DagError::Schedule("STRING PPI enrichment returned no result".to_owned())
        })?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("nodes", DataType::UInt64, false),
                Field::new("edges", DataType::UInt64, false),
                Field::new("average_node_degree", DataType::Float64, false),
                Field::new("local_clustering_coefficient", DataType::Float64, false),
                Field::new("expected_edges", DataType::Float64, false),
                Field::new("p_value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(UInt64Array::from(vec![Some(row.number_of_nodes)])),
                Arc::new(UInt64Array::from(vec![Some(row.number_of_edges)])),
                Arc::new(Float64Array::from(vec![Some(row.average_node_degree)])),
                Arc::new(Float64Array::from(vec![Some(
                    row.local_clustering_coefficient,
                )])),
                Arc::new(Float64Array::from(vec![Some(row.expected_number_of_edges)])),
                Arc::new(Float64Array::from(vec![Some(row.p_value)])),
            ],
        )
        .map_err(|error| {
            DagError::Schedule(format!(
                "failed to build STRING PPI enrichment batch: {error}"
            ))
        })?;
        let df = read_batch(ctx, batch, "PPI enrichment").await?;
        output(df)
    }
}
