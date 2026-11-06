//! Open Targets Platform source nodes.
//!
//! Five nodes that pull tabular data from the [Open Targets Platform](
//! https://platform.opentargets.org/api) GraphQL API and emit it as a
//! DataFusion `DataFrame`, so association score tables and search hits can
//! flow through a DAG (join, filter via `sql_node`, sink to CSV/VFS, …).
//!
//! - [`OpentargetsAssociationsNode`] (`source_opentargets_associations`) —
//!   target↔disease association score table.
//! - [`OpentargetsSearchNode`] (`source_opentargets_search`) — full-text
//!   search hits table (resolve names → stable IDs inside a pipeline).
//! - [`OpentargetsAssociatedDiseasesNode`]
//!   (`source_opentargets_associated_diseases`) — the ranked association
//!   table with per-datasource / per-datatype score columns.
//! - [`OpentargetsAssociatedTargetsNode`]
//!   (`source_opentargets_associated_targets`) — the disease→target
//!   counterpart with the same score breakdown.
//! - [`OpentargetsGeneticNode`] (`opentargets_genetic`) — fine-mapped
//!   credible sets with Locus-to-Gene predictions, IDs read from an
//!   upstream DataFrame (list input stays out of the spec).
//!
//! The first four are zero-input / single-output source nodes; the genetics
//! node has one DataFrame input and one output. All reuse the `opentargets`
//! SDK client; `DagNode::execute` is async on the engine's tokio runtime, so
//! the client is simply `.await`ed.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use opentargets::{
    AssociatedDisease, AssociatedTarget, CredibleSetLocus, OpenTargetsClient, Pagination,
    ScoredComponent, SearchResult,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build an [`OpenTargetsClient`] from an optional endpoint override.
fn client_from_endpoint(endpoint: &Option<String>) -> OpenTargetsClient {
    match endpoint {
        Some(url) => OpenTargetsClient::with_endpoint(url),
        None => OpenTargetsClient::new(),
    }
}

/// Turn a `RecordBatch` into a `DataFrame` via a fresh isolated session.
fn batch_to_df(ctx: &SessionContext, batch: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read Open Targets batch: {e}")))
}

/// Build a nullable `StringArray` from `Vec<Option<String>>`.
fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}

/// Build a nullable `Float64Array` from `Vec<Option<f64>>`.
fn f64_array(rows: Vec<Option<f64>>) -> Arc<dyn Array> {
    Arc::new(Float64Array::from(rows))
}

fn field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::Utf8, nullable)
}

// ===========================================================================
// 1. Associations node
// ===========================================================================

/// Spec for [`OpentargetsAssociationsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsAssociationsSpec {
    /// Direction of the association: `"target_to_disease"` (default) or
    /// `"disease_to_target"`.
    #[serde(default)]
    pub direction: Option<String>,
    /// Ensembl gene ID (for `target_to_disease`) or ontology ID such as a
    /// MONDO/EFO code (for `disease_to_target`).
    pub id: String,
    /// Include indirect (propagated) associations. Default `false`.
    #[serde(default)]
    pub enable_indirect: Option<bool>,
    /// Server-side entity-name filter (e.g. `"cancer"`).
    #[serde(default)]
    pub b_filter: Option<String>,
    /// Client-side minimum overall score in `[0,1]`; rows below are dropped.
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Auto-paginate **all** associations. Default `true`.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Page size when `fetch_all` is `false` (default 25, max 3000).
    #[serde(default)]
    pub size: Option<u32>,
    /// 0-based page index when `fetch_all` is `false`.
    #[serde(default)]
    pub index: Option<u32>,
    /// Override the GraphQL endpoint URL (tests / staging).
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Source node emitting a target↔disease association score table.
#[derive(Clone)]
pub struct OpentargetsAssociationsNode {
    meta: NodePorts,
    spec: OpentargetsAssociationsSpec,
}

pub struct OpentargetsAssociationsNodeFactory {}

fn assoc_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for OpentargetsAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opentargets_associations"
    }

    fn desc(&self) -> &'static str {
        "Fetches target↔disease association scores from the Open Targets Platform as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that fetches target–disease (or disease–target) association \
        scores from the Open Targets Platform GraphQL API and emits them as a DataFrame. \
        No input ports; one output port.\n\n\
        Set `direction` to `\"target_to_disease\"` (default) with `id` = Ensembl gene ID, \
        or `\"disease_to_target\"` with `id` = ontology ID (e.g. MONDO_0004975).\n\n\
        Output schema (target_to_disease): \
        `target_id, disease_id, disease_name, score, novelty`.\n\
        Output schema (disease_to_target): \
        `disease_id, target_id, symbol, approved_name, biotype, score, novelty`.\n\n\
        By default `fetch_all=true` retrieves every association across pages. \
        Use `min_score` / `b_filter` to constrain the result, then pipe into \
        `sql_node` or `dataframe_to_file`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsAssociationsSpec)
    }

    fn ports(&self) -> NodePorts {
        assoc_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpentargetsAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpentargetsAssociationsNode {
            meta: assoc_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpentargetsAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_opentargets_associations"
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
        let direction = self
            .spec
            .direction
            .clone()
            .unwrap_or_else(|| "target_to_disease".to_string());
        let enable_indirect = self.spec.enable_indirect.unwrap_or(false);
        let b_filter = self.spec.b_filter.as_deref();
        let min_score = self.spec.min_score.unwrap_or(0.0);
        let client = client_from_endpoint(&self.spec.endpoint);
        let session = ctx.session();

        let batch = match direction.as_str() {
            "target_to_disease" => {
                let rows =
                    assoc_disease_rows(&client, &self.spec, enable_indirect, b_filter).await?;
                build_disease_batch(&session, &self.spec.id, rows, min_score)?
            }
            "disease_to_target" => {
                let rows =
                    assoc_target_rows(&client, &self.spec, enable_indirect, b_filter).await?;
                build_target_batch(&session, &self.spec.id, rows, min_score)?
            }
            other => {
                return Err(DagError::Schedule(format!(
                    "unknown direction '{other}' (expected 'target_to_disease' or 'disease_to_target')"
                )));
            }
        };

        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

async fn assoc_disease_rows(
    client: &OpenTargetsClient,
    spec: &OpentargetsAssociationsSpec,
    enable_indirect: bool,
    b_filter: Option<&str>,
) -> Result<Vec<AssociatedDisease>, DagError> {
    let map_err = |e: opentargets::OpenTargetsError| {
        DagError::Schedule(format!("Open Targets request failed: {e}"))
    };
    let rows = if spec.fetch_all.unwrap_or(true) {
        client
            .associated_diseases_all_filtered(&spec.id, enable_indirect, b_filter)
            .await
            .map_err(map_err)?
    } else {
        client
            .associated_diseases(
                &spec.id,
                Pagination::new(spec.index.unwrap_or(0), spec.size.unwrap_or(25).min(3000)),
                enable_indirect,
                b_filter,
            )
            .await
            .map_err(map_err)?
            .rows
    };
    Ok(rows)
}

async fn assoc_target_rows(
    client: &OpenTargetsClient,
    spec: &OpentargetsAssociationsSpec,
    enable_indirect: bool,
    b_filter: Option<&str>,
) -> Result<Vec<AssociatedTarget>, DagError> {
    let map_err = |e: opentargets::OpenTargetsError| {
        DagError::Schedule(format!("Open Targets request failed: {e}"))
    };
    let rows = if spec.fetch_all.unwrap_or(true) {
        client
            .associated_targets_all_filtered(&spec.id, enable_indirect, b_filter)
            .await
            .map_err(map_err)?
    } else {
        client
            .associated_targets(
                &spec.id,
                Pagination::new(spec.index.unwrap_or(0), spec.size.unwrap_or(25).min(3000)),
                enable_indirect,
                b_filter,
            )
            .await
            .map_err(map_err)?
            .rows
    };
    Ok(rows)
}

/// Build the target→disease batch: `target_id, disease_id, disease_name, score, novelty`.
fn build_disease_batch(
    _session: &SessionContext,
    target_id: &str,
    mut rows: Vec<AssociatedDisease>,
    min_score: f64,
) -> Result<RecordBatch, DagError> {
    if min_score > 0.0 {
        rows.retain(|a| a.score >= min_score);
    }
    let n = rows.len();
    let target_ids = vec![Some(target_id.to_string()); n];
    let disease_ids: Vec<Option<String>> =
        rows.iter().map(|a| Some(a.disease.id.clone())).collect();
    let disease_names: Vec<Option<String>> =
        rows.iter().map(|a| Some(a.disease.name.clone())).collect();
    let scores: Vec<Option<f64>> = rows.iter().map(|a| Some(a.score)).collect();
    let novelties: Vec<Option<f64>> = rows.iter().map(|a| a.novelty).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("target_id", DataType::Utf8, true),
        Field::new("disease_id", DataType::Utf8, true),
        Field::new("disease_name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("novelty", DataType::Float64, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(target_ids),
            str_array(disease_ids),
            str_array(disease_names),
            f64_array(scores),
            f64_array(novelties),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build association batch: {e}")))
}

/// Build the disease→target batch:
/// `disease_id, target_id, symbol, approved_name, biotype, score, novelty`.
fn build_target_batch(
    _session: &SessionContext,
    disease_id: &str,
    mut rows: Vec<AssociatedTarget>,
    min_score: f64,
) -> Result<RecordBatch, DagError> {
    if min_score > 0.0 {
        rows.retain(|a| a.score >= min_score);
    }
    let n = rows.len();
    let disease_ids = vec![Some(disease_id.to_string()); n];
    let target_ids: Vec<Option<String>> = rows.iter().map(|a| Some(a.target.id.clone())).collect();
    let symbols: Vec<Option<String>> = rows
        .iter()
        .map(|a| Some(a.target.approved_symbol.clone()))
        .collect();
    let names: Vec<Option<String>> = rows
        .iter()
        .map(|a| Some(a.target.approved_name.clone()))
        .collect();
    let biotypes: Vec<Option<String>> = rows
        .iter()
        .map(|a| Some(a.target.biotype.clone()))
        .collect();
    let scores: Vec<Option<f64>> = rows.iter().map(|a| Some(a.score)).collect();
    let novelties: Vec<Option<f64>> = rows.iter().map(|a| a.novelty).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("disease_id", DataType::Utf8, true),
        Field::new("target_id", DataType::Utf8, true),
        Field::new("symbol", DataType::Utf8, true),
        Field::new("approved_name", DataType::Utf8, true),
        Field::new("biotype", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("novelty", DataType::Float64, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(disease_ids),
            str_array(target_ids),
            str_array(symbols),
            str_array(names),
            str_array(biotypes),
            f64_array(scores),
            f64_array(novelties),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build association batch: {e}")))
}

// ===========================================================================
// 2. Search node
// ===========================================================================

/// Spec for [`OpentargetsSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsSearchSpec {
    /// Free-text query, e.g. `"BRCA1"` or `"Alzheimer"`.
    pub query: String,
    /// Optional entity filters: `target`, `disease`, `drug`, `study`, `variant`.
    #[serde(default)]
    pub entity: Option<Vec<String>>,
    /// Page size (default 50, max 3000).
    #[serde(default)]
    pub size: Option<u32>,
    /// 0-based page index (default 0).
    #[serde(default)]
    pub index: Option<u32>,
    /// Override the GraphQL endpoint URL (tests / staging).
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Source node emitting an Open Targets search-hits table.
#[derive(Clone)]
pub struct OpentargetsSearchNode {
    meta: NodePorts,
    spec: OpentargetsSearchSpec,
}

pub struct OpentargetsSearchNodeFactory {}

fn search_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for OpentargetsSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opentargets_search"
    }

    fn desc(&self) -> &'static str {
        "Searches the Open Targets Platform and emits ranked hits as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that runs a full-text search across the Open Targets Platform \
        and emits the ranked hits as a DataFrame. Useful for resolving an entity name \
        into its stable ID (Ensembl / EFO / ChEMBL / GCST) inside a pipeline.\n\n\
        Output schema: `entity, id, name, score, description`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsSearchSpec)
    }

    fn ports(&self) -> NodePorts {
        search_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpentargetsSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpentargetsSearchNode {
            meta: search_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpentargetsSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_opentargets_search"
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
        let client = client_from_endpoint(&self.spec.endpoint);
        let entities: Option<Vec<&str>> = self
            .spec
            .entity
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let results = client
            .search(
                &self.spec.query,
                entities.as_deref(),
                Some(Pagination::new(
                    self.spec.index.unwrap_or(0),
                    self.spec.size.unwrap_or(50).min(3000),
                )),
            )
            .await
            .map_err(|e| DagError::Schedule(format!("Open Targets search failed: {e}")))?;

        let session = ctx.session();
        let batch = build_search_batch(results.hits)?;
        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the search batch: `entity, id, name, score, description`.
fn build_search_batch(rows: Vec<SearchResult>) -> Result<RecordBatch, DagError> {
    let entities: Vec<Option<String>> = rows.iter().map(|h| Some(h.entity.clone())).collect();
    let ids: Vec<Option<String>> = rows.iter().map(|h| Some(h.id.clone())).collect();
    let names: Vec<Option<String>> = rows.iter().map(|h| Some(h.name.clone())).collect();
    let scores: Vec<Option<f64>> = rows.iter().map(|h| Some(h.score)).collect();
    let descriptions: Vec<Option<String>> = rows.iter().map(|h| h.description.clone()).collect();

    let schema = Arc::new(Schema::new(vec![
        field("entity", true),
        field("id", true),
        field("name", true),
        Field::new("score", DataType::Float64, true),
        field("description", true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(entities),
            str_array(ids),
            str_array(names),
            f64_array(scores),
            str_array(descriptions),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build search batch: {e}")))
}

// ===========================================================================
// 3. Associated diseases node (target → ranked diseases + score breakdown)
// ===========================================================================

/// Spec for [`OpentargetsAssociatedDiseasesNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsAssociatedDiseasesSpec {
    /// Ensembl gene ID of the target, e.g. `"ENSG00000139618"`.
    pub ensembl_id: String,
    /// Include indirect (propagated) associations. Default `false`.
    #[serde(default)]
    pub enable_indirect: Option<bool>,
    /// Server-side entity-name filter (e.g. `"cancer"`).
    #[serde(default)]
    pub b_filter: Option<String>,
    /// Client-side minimum overall score in `[0,1]`; rows below are dropped.
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Auto-paginate **all** associations. Default `true`.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Page size when `fetch_all` is `false` (default 25, max 3000).
    #[serde(default)]
    pub size: Option<u32>,
    /// 0-based page index when `fetch_all` is `false`.
    #[serde(default)]
    pub index: Option<u32>,
    /// Top N datasources to un-pivot into `ds_<id>` score columns
    /// (by best score across rows; 0 = none; default 3).
    #[serde(default)]
    pub top_datasources: Option<u32>,
    /// Top N datatypes to un-pivot into `dt_<id>` score columns
    /// (0 = none, default 3).
    #[serde(default)]
    pub top_datatypes: Option<u32>,
    /// Override the GraphQL endpoint URL (tests / staging).
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Source node emitting the target→disease ranked table with per-datasource
/// and per-datatype score columns.
#[derive(Clone)]
pub struct OpentargetsAssociatedDiseasesNode {
    meta: NodePorts,
    spec: OpentargetsAssociatedDiseasesSpec,
}

pub struct OpentargetsAssociatedDiseasesNodeFactory;

impl NodeFactory for OpentargetsAssociatedDiseasesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opentargets_associated_diseases"
    }

    fn desc(&self) -> &'static str {
        "Ranked diseases for a target with per-datasource/datatype score columns."
    }

    fn doc(&self) -> &'static str {
        "A source node over Open Targets `target.associatedDiseases` — the \
         pipeline form of the `opentargets_associated_diseases` tool. Every \
         row carries the overall score plus the per-datasource and \
         per-datatype breakdown un-pivoted into extra columns \
         (`ds_genetic_association`, `dt_known_drug`, …), so ranking and \
         filtering by evidence type happens in `sql` instead of by \
         re-querying.\n\n\
         `top_datasources` / `top_datatypes` (default 3 each) pick which \
         score columns to emit; the top-N ids are chosen by best score across \
         rows (score desc, then id) so the schema stays stable per input.\
         \n\n\
         Output schema: `disease_id, disease_name, score, novelty, \
         ds_<id>…, dt_<id>…`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsAssociatedDiseasesSpec)
    }

    fn ports(&self) -> NodePorts {
        assoc_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpentargetsAssociatedDiseasesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpentargetsAssociatedDiseasesNode {
            meta: assoc_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpentargetsAssociatedDiseasesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_opentargets_associated_diseases"
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
        let client = client_from_endpoint(&self.spec.endpoint);
        let indirect = self.spec.enable_indirect.unwrap_or(false);
        let b_filter = self.spec.b_filter.as_deref();
        let map_err = |e: opentargets::OpenTargetsError| {
            DagError::Schedule(format!("Open Targets request failed: {e}"))
        };
        let mut rows: Vec<AssociatedDisease> = if self.spec.fetch_all.unwrap_or(true) {
            client
                .associated_diseases_all_filtered(&self.spec.ensembl_id, indirect, b_filter)
                .await
                .map_err(map_err)?
        } else {
            client
                .associated_diseases(
                    &self.spec.ensembl_id,
                    Pagination::new(
                        self.spec.index.unwrap_or(0),
                        self.spec.size.unwrap_or(25).min(3000),
                    ),
                    indirect,
                    b_filter,
                )
                .await
                .map_err(map_err)?
                .rows
        };
        if let Some(min_score) = self.spec.min_score {
            if min_score > 0.0 {
                rows.retain(|a| a.score >= min_score);
            }
        }
        let batch = build_breakdown_batch(
            rows.iter().map(|a| BreakdownRow {
                id: &a.disease.id,
                name: &a.disease.name,
                extra: ["", ""],
                score: a.score,
                novelty: a.novelty,
                datasources: &a.datasource_scores,
                datatypes: &a.datatype_scores,
            }),
            self.spec.top_datasources.unwrap_or(3) as usize,
            self.spec.top_datatypes.unwrap_or(3) as usize,
            "disease_id",
            "disease_name",
        )?;
        let df = batch_to_df(&ctx.session(), batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ===========================================================================
// 4. Associated targets node (disease → ranked targets + score breakdown)
// ===========================================================================

/// Spec for [`OpentargetsAssociatedTargetsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsAssociatedTargetsSpec {
    /// EFO/MONDO ontology ID of the disease, e.g. `"EFO_0000249"`.
    pub efo_id: String,
    /// Include indirect (propagated) associations. Default `false`.
    #[serde(default)]
    pub enable_indirect: Option<bool>,
    /// Server-side entity-name filter (e.g. `"kinase"`).
    #[serde(default)]
    pub b_filter: Option<String>,
    /// Client-side minimum overall score in `[0,1]`.
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Auto-paginate **all** associations. Default `true`.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Page size when `fetch_all` is `false` (default 25, max 3000).
    #[serde(default)]
    pub size: Option<u32>,
    /// 0-based page index when `fetch_all` is `false`.
    #[serde(default)]
    pub index: Option<u32>,
    /// Top N datasources to un-pivot into score columns (default 3).
    #[serde(default)]
    pub top_datasources: Option<u32>,
    /// Top N datatypes to un-pivot into score columns (default 3).
    #[serde(default)]
    pub top_datatypes: Option<u32>,
    /// Override the GraphQL endpoint URL (tests / staging).
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Source node emitting the disease→target ranked table with score breakdown.
#[derive(Clone)]
pub struct OpentargetsAssociatedTargetsNode {
    meta: NodePorts,
    spec: OpentargetsAssociatedTargetsSpec,
}

pub struct OpentargetsAssociatedTargetsNodeFactory;

impl NodeFactory for OpentargetsAssociatedTargetsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opentargets_associated_targets"
    }

    fn desc(&self) -> &'static str {
        "Ranked targets for a disease with per-datasource/datatype score columns."
    }

    fn doc(&self) -> &'static str {
        "A source node over Open Targets `disease.associatedTargets` — the \
         disease→target counterpart of \
         `source_opentargets_associated_diseases`, with the same \
         per-datasource / per-datatype score columns.\n\n\
         Output schema: `target_id, symbol, approved_name, biotype, score, \
         novelty, ds_<id>…, dt_<id>…`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsAssociatedTargetsSpec)
    }

    fn ports(&self) -> NodePorts {
        assoc_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpentargetsAssociatedTargetsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpentargetsAssociatedTargetsNode {
            meta: assoc_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpentargetsAssociatedTargetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_opentargets_associated_targets"
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
        let client = client_from_endpoint(&self.spec.endpoint);
        let indirect = self.spec.enable_indirect.unwrap_or(false);
        let b_filter = self.spec.b_filter.as_deref();
        let map_err = |e: opentargets::OpenTargetsError| {
            DagError::Schedule(format!("Open Targets request failed: {e}"))
        };
        let mut rows: Vec<AssociatedTarget> = if self.spec.fetch_all.unwrap_or(true) {
            client
                .associated_targets_all_filtered(&self.spec.efo_id, indirect, b_filter)
                .await
                .map_err(map_err)?
        } else {
            client
                .associated_targets(
                    &self.spec.efo_id,
                    Pagination::new(
                        self.spec.index.unwrap_or(0),
                        self.spec.size.unwrap_or(25).min(3000),
                    ),
                    indirect,
                    b_filter,
                )
                .await
                .map_err(map_err)?
                .rows
        };
        if let Some(min_score) = self.spec.min_score {
            if min_score > 0.0 {
                rows.retain(|a| a.score >= min_score);
            }
        }
        let batch = build_breakdown_batch(
            rows.iter().map(|a| BreakdownRow {
                id: &a.target.id,
                name: &a.target.approved_symbol,
                extra: [a.target.approved_name.as_str(), a.target.biotype.as_str()],
                score: a.score,
                novelty: a.novelty,
                datasources: &a.datasource_scores,
                datatypes: &a.datatype_scores,
            }),
            self.spec.top_datasources.unwrap_or(3) as usize,
            self.spec.top_datatypes.unwrap_or(3) as usize,
            "target_id",
            "symbol",
        )?;
        let df = batch_to_df(&ctx.session(), batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ===========================================================================
// 5. Genetics node (credible sets + Locus-to-Gene, IDs from an input frame)
// ===========================================================================

/// Default name of the input column holding variant/study IDs.
fn default_genetic_id_column() -> String {
    "variant_id".into()
}

/// Default number of IDs per `credibleSets` request.
fn default_genetic_batch_size() -> u32 {
    25
}

/// Page size for the internal `credibleSets` pagination loop.
const GENETIC_PAGE_SIZE: u32 = 100;

/// Spec for [`OpentargetsGeneticNode`]. No list fields: the ID list always
/// arrives on the input DataFrame port.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsGeneticSpec {
    /// What the input column holds: `"variant"` (default) — `chr_pos_ref_alt`
    /// GRCh38 lead variants — or `"study"` — GWAS/QTL study IDs (GCST…).
    #[serde(default)]
    pub id_kind: Option<String>,
    /// Name of the input DataFrame column holding the IDs. Default
    /// `variant_id`.
    #[serde(default = "default_genetic_id_column")]
    pub id_column: String,
    /// Minimum Locus-to-Gene score in `[0,1]`; gene rows below are dropped.
    /// Default keeps every predicted gene.
    #[serde(default)]
    pub min_l2g: Option<f64>,
    /// Legacy chunk size; requests are issued per ID to preserve attribution.
    /// Default `25`.
    #[serde(default = "default_genetic_batch_size")]
    pub batch_size: u32,
    /// Override the GraphQL endpoint URL (tests / staging).
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Genetics node: fine-mapped credible sets with Locus-to-Gene predictions,
/// the IDs read from an upstream DataFrame column (phewas-style).
#[derive(Clone)]
pub struct OpentargetsGeneticNode {
    meta: NodePorts,
    spec: OpentargetsGeneticSpec,
}

pub struct OpentargetsGeneticNodeFactory;

impl NodeFactory for OpentargetsGeneticNodeFactory {
    fn kind(&self) -> &'static str {
        "opentargets_genetic"
    }

    fn desc(&self) -> &'static str {
        "Fine-mapped credible sets with Locus-to-Gene predictions for variants \
        or studies read from an upstream DataFrame."
    }

    fn doc(&self) -> &'static str {
        "Takes a DataFrame with an ID column (`id_column`, default `variant_id`) \
         and queries the Open Targets Platform `credibleSets` endpoint — the \
         genetics_portal content (fine-mapping + Locus-to-Gene) now lives in \
         the Platform API. `id_kind` selects the column semantics: `\"variant\"` \
         (default, `chr_pos_ref_alt` GRCh38 lead variants) or `\"study\"` \
         (GCST-style study IDs).\n\n\
         Output is long-format, one row per (locus, predicted gene): \
         `input_id, study_locus_id, study_id, trait, study_type, \
         lead_variant_id, lead_variant_rsid, pval, beta, purity_r2, gene_id, \
         gene_symbol, l2g_score`. Loci without any predicted gene (above \
         `min_l2g`) are dropped.\n\n\
         Note: in variant mode loci whose LD set contains the queried variant \
         are also returned; `lead_variant_id` carries the actual lead variant, \
         and `input_id` preserves the queried ID for joins back to the input."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsGeneticSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port_of_type_with_label(None, PortType::DataFrame, "ids")
            .add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpentargetsGeneticSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpentargetsGeneticNode {
            meta: self.ports(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpentargetsGeneticNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "opentargets_genetic"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let err = |msg: String| DagError::NodeError {
            node_type: "opentargets_genetic".into(),
            msg,
        };
        let kind = self.spec.id_kind.as_deref().unwrap_or("variant");
        let fetch = match kind {
            "variant" | "study" => kind,
            other => {
                return Err(err(format!(
                    "unknown id_kind '{other}' (expected 'variant' or 'study')"
                )));
            }
        };

        let input = inputs.first().ok_or_else(|| {
            err(format!(
                "no input: connect a DataFrame with {} IDs to the 'ids' port",
                if fetch == "variant" {
                    "variant"
                } else {
                    "study"
                }
            ))
        })?;
        let df = input
            .dataframe()
            .map_err(|e| err(format!("input is not a DataFrame: {e}")))?;
        let ids = genetic_string_values(&df, &self.spec.id_column).await?;
        if ids.is_empty() {
            return Err(err(format!(
                "column '{}' contains no usable IDs",
                self.spec.id_column
            )));
        }
        tracing::info!(
            "Open Targets credibleSets: querying {} unique {fetch} IDs (min_l2g = {})",
            ids.len(),
            self.spec.min_l2g.unwrap_or(0.0),
        );

        let client = client_from_endpoint(&self.spec.endpoint);
        let min_l2g = self.spec.min_l2g.unwrap_or(0.0);
        let batch = self.spec.batch_size.max(1) as usize;
        let mut loci: Vec<(String, CredibleSetLocus)> = Vec::new();
        for chunk in ids.chunks(batch).flat_map(|chunk| chunk.chunks(1)) {
            let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let mut index = 0u32;
            let mut fetched = 0i64;
            loop {
                let page = if fetch == "variant" {
                    client
                        .credible_sets_by_variant(&refs, Pagination::new(index, GENETIC_PAGE_SIZE))
                        .await
                } else {
                    client
                        .credible_sets_by_study(&refs, Pagination::new(index, GENETIC_PAGE_SIZE))
                        .await
                }
                .map_err(|e| DagError::Schedule(format!("Open Targets request failed: {e}")))?;
                let count = page.count;
                let got = page.rows.len() as i64;
                fetched += got;
                loci.extend(page.rows.into_iter().map(|locus| (chunk[0].clone(), locus)));
                if fetched >= count || got == 0 {
                    break;
                }
                index += 1;
            }
        }

        let batch = build_genetic_batch(loci, min_l2g)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read genetics batch: {e}")))?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Extract non-null string values from one column of a DataFrame, deduped in
/// first-seen order (duplicate IDs only multiply network requests).
async fn genetic_string_values(df: &DataFrame, column: &str) -> Result<Vec<String>, DagError> {
    let index = df
        .schema()
        .index_of_column_by_name(None, column)
        .ok_or_else(|| DagError::NodeError {
            node_type: "opentargets_genetic".into(),
            msg: format!("input table has no column {column:?}"),
        })?;
    let batches = df
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::Schedule(format!("failed to collect input table: {e}")))?;
    let mut seen = std::collections::HashSet::new();
    let mut values = Vec::new();
    for batch in batches {
        let array = batch.column(index);
        let strings = crate::arrow_strings::utf8_strings(array).ok_or_else(|| DagError::NodeError {
            node_type: "opentargets_genetic".into(),
            msg: format!(
                "column {column:?} must be UTF-8 (found {})",
                array.data_type()
            ),
        })?;
        for value in strings.into_iter().flatten() {
            if seen.insert(value.to_string()) {
                values.push(value.to_string());
            }
        }
    }
    Ok(values)
}

/// One long-format output row: a credible-set locus crossed with one
/// predicted gene.
struct GeneticRow {
    input_id: String,
    study_locus_id: String,
    study_id: String,
    trait_name: Option<String>,
    study_type: Option<String>,
    lead_variant_id: Option<String>,
    lead_variant_rsid: Option<String>,
    pval: Option<f64>,
    beta: Option<f64>,
    purity_r2: Option<f64>,
    gene_id: String,
    gene_symbol: Option<String>,
    l2g_score: f64,
}

/// Flatten `(input_id, locus)` pairs into gene rows, dropping loci whose
/// predictions all fall below `min_l2g`.
fn genetic_rows(
    loci: Vec<(String, CredibleSetLocus)>,
    min_l2g: f64,
) -> impl Iterator<Item = GeneticRow> {
    loci.into_iter().flat_map(move |(input_id, locus)| {
        let study = locus.study.clone();
        let variant = locus.variant.clone();
        let pval = match (locus.p_value_mantissa, locus.p_value_exponent) {
            (Some(m), Some(e)) => Some(m * 10f64.powi(e)),
            _ => None,
        };
        locus
            .l2g_predictions
            .rows
            .into_iter()
            .filter(move |p| p.score >= min_l2g)
            .map(move |p| GeneticRow {
                input_id: input_id.clone(),
                study_locus_id: locus.study_locus_id.clone(),
                study_id: locus.study_id.clone(),
                trait_name: study.as_ref().and_then(|s| s.trait_from_source.clone()),
                study_type: locus.study_type.clone(),
                lead_variant_id: variant.as_ref().and_then(|v| v.id.clone()),
                lead_variant_rsid: variant
                    .as_ref()
                    .and_then(|v| v.rs_ids.as_ref().and_then(|r| r.first().cloned())),
                pval,
                beta: locus.beta,
                purity_r2: locus.purity_mean_r2,
                gene_id: p.target.id.clone(),
                gene_symbol: p.target.approved_symbol.clone(),
                l2g_score: p.score,
            })
    })
}

/// Assemble the genetics output batch:
/// `input_id, study_locus_id, study_id, trait, study_type, lead_variant_id,
/// lead_variant_rsid, pval, beta, purity_r2, gene_id, gene_symbol, l2g_score`.
fn build_genetic_batch(
    loci: Vec<(String, CredibleSetLocus)>,
    min_l2g: f64,
) -> Result<RecordBatch, DagError> {
    let rows: Vec<GeneticRow> = genetic_rows(loci, min_l2g).collect();
    let schema = Arc::new(Schema::new(vec![
        field("input_id", true),
        field("study_locus_id", true),
        field("study_id", true),
        field("trait", true),
        field("study_type", true),
        field("lead_variant_id", true),
        field("lead_variant_rsid", true),
        Field::new("pval", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
        Field::new("purity_r2", DataType::Float64, true),
        field("gene_id", true),
        field("gene_symbol", true),
        Field::new("l2g_score", DataType::Float64, true),
    ]));
    let columns = vec![
        str_array(rows.iter().map(|r| Some(r.input_id.clone())).collect()),
        str_array(
            rows.iter()
                .map(|r| Some(r.study_locus_id.clone()))
                .collect(),
        ),
        str_array(rows.iter().map(|r| Some(r.study_id.clone())).collect()),
        str_array(rows.iter().map(|r| r.trait_name.clone()).collect()),
        str_array(rows.iter().map(|r| r.study_type.clone()).collect()),
        str_array(rows.iter().map(|r| r.lead_variant_id.clone()).collect()),
        str_array(rows.iter().map(|r| r.lead_variant_rsid.clone()).collect()),
        f64_array(rows.iter().map(|r| r.pval).collect()),
        f64_array(rows.iter().map(|r| r.beta).collect()),
        f64_array(rows.iter().map(|r| r.purity_r2).collect()),
        str_array(rows.iter().map(|r| Some(r.gene_id.clone())).collect()),
        str_array(rows.iter().map(|r| r.gene_symbol.clone()).collect()),
        f64_array(rows.iter().map(|r| Some(r.l2g_score)).collect()),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|e| DagError::Schedule(format!("failed to build genetics batch: {e}")))
}

// ---------------------------------------------------------------------------
// Shared breakdown batch builder
// ---------------------------------------------------------------------------

/// One row projected out of either association row type, so the breakdown
/// builder stays generic over the two SDK structs.
struct BreakdownRow<'a> {
    /// Stable entity id (first column).
    id: &'a str,
    /// Disease name or target symbol (second column).
    name: &'a str,
    /// Target-only identity fields (`approved_name`, `biotype`); empty for
    /// the disease projection. A fixed array (not a slice) so the projection
    /// borrows straight from the SDK row without a temporary.
    extra: [&'a str; 2],
    score: f64,
    novelty: Option<f64>,
    datasources: &'a [ScoredComponent],
    datatypes: &'a [ScoredComponent],
}

/// Assemble the ranked batch: `id_col, name_col, score, novelty`, plus the
/// target-only extra columns, plus the top-N `ds_<id>` / `dt_<id>`
/// un-pivoted score columns.
fn build_breakdown_batch<'a>(
    rows: impl ExactSizeIterator<Item = BreakdownRow<'a>>,
    top_datasources: usize,
    top_datatypes: usize,
    id_col: &str,
    name_col: &str,
) -> Result<RecordBatch, DagError> {
    let material: Vec<BreakdownRow<'a>> = rows.collect();
    let n = material.len();

    // Top-N component ids by best score across rows, deterministic order
    // (score desc, then id) so equal scores do not reshuffle between runs.
    fn top_ids<'b>(groups: &[&'b [ScoredComponent]], top: usize) -> Vec<&'b str> {
        let mut best: std::collections::HashMap<&'b str, f64> = std::collections::HashMap::new();
        for group in groups {
            for component in *group {
                let entry = best.entry(component.id.as_str()).or_insert(component.score);
                if component.score > *entry {
                    *entry = component.score;
                }
            }
        }
        let mut ranked: Vec<(&str, f64)> = best.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        ranked.truncate(top);
        ranked.into_iter().map(|(id, _)| id).collect()
    }

    let ds_groups: Vec<&[ScoredComponent]> = material.iter().map(|r| r.datasources).collect();
    let dt_groups: Vec<&[ScoredComponent]> = material.iter().map(|r| r.datatypes).collect();
    let ds_ids = top_ids(&ds_groups, top_datasources);
    let dt_ids = top_ids(&dt_groups, top_datatypes);
    let component_column = |groups: &[&[ScoredComponent]], id: &str| -> Vec<Option<f64>> {
        (0..n)
            .map(|i| {
                groups
                    .get(i)
                    .and_then(|group| group.iter().find(|c| c.id == id))
                    .map(|c| c.score)
            })
            .collect()
    };

    // The disease projection passes `["", ""]`; the target projection names
    // its two extra columns. Empty first slot means "no extra columns".
    let extra_names: [&str; 2] = material.first().map(|r| r.extra).unwrap_or(["", ""]);
    let extra_active = !extra_names[0].is_empty();
    let mut fields = vec![
        field(id_col, true),
        field(name_col, true),
        Field::new("score", DataType::Float64, true),
        Field::new("novelty", DataType::Float64, true),
    ];
    if extra_active {
        for extra in extra_names {
            fields.push(field(extra, true));
        }
    }
    for id in &ds_ids {
        fields.push(Field::new(format!("ds_{id}"), DataType::Float64, true));
    }
    for id in &dt_ids {
        fields.push(Field::new(format!("dt_{id}"), DataType::Float64, true));
    }
    let schema = Arc::new(Schema::new(fields));

    let mut columns: Vec<Arc<dyn Array>> = vec![
        str_array((0..n).map(|i| Some(material[i].id.to_string())).collect()),
        str_array((0..n).map(|i| Some(material[i].name.to_string())).collect()),
        f64_array((0..n).map(|i| Some(material[i].score)).collect()),
        f64_array((0..n).map(|i| material[i].novelty).collect()),
    ];
    if extra_active {
        for slot in 0..extra_names.len() {
            columns.push(str_array(
                (0..n)
                    .map(|i| Some(material[i].extra[slot].to_string()))
                    .collect(),
            ));
        }
    }
    for id in &ds_ids {
        columns.push(f64_array(component_column(&ds_groups, id)));
    }
    for id in &dt_ids {
        columns.push(f64_array(component_column(&dt_groups, id)));
    }

    RecordBatch::try_new(schema, columns)
        .map_err(|e| DagError::Schedule(format!("failed to build breakdown batch: {e}")))
}

#[cfg(test)]
mod breakdown_tests {
    use super::*;

    fn component(id: &str, score: f64) -> ScoredComponent {
        ScoredComponent {
            id: id.into(),
            score,
        }
    }

    fn row<'a>(
        id: &'a str,
        name: &'a str,
        extra: [&'a str; 2],
        score: f64,
        novelty: Option<f64>,
        ds: &'a [ScoredComponent],
        dt: &'a [ScoredComponent],
    ) -> BreakdownRow<'a> {
        BreakdownRow {
            id,
            name,
            extra,
            score,
            novelty,
            datasources: ds,
            datatypes: dt,
        }
    }

    #[test]
    fn breakdown_unpivots_top_components_with_novelty() {
        let ds0 = vec![component("gwas", 0.8), component("ot_genetics", 0.7)];
        let ds1 = vec![component("gwas", 0.4)];
        let dt0 = vec![component("genetic_association", 0.8)];
        let dt1 = vec![component("known_drug", 0.5)];
        let rows = vec![
            row("EFO_1", "asthma", ["", ""], 0.9, Some(0.1), &ds0, &dt0),
            row("EFO_2", "cancer", ["", ""], 0.5, None, &ds1, &dt1),
        ];
        let batch =
            build_breakdown_batch(rows.into_iter(), 1, 1, "disease_id", "disease_name").unwrap();
        assert_eq!(batch.num_rows(), 2);
        // disease_id, disease_name, score, novelty, ds_gwas, dt_genetic_association
        assert_eq!(batch.num_columns(), 6);
        // Top datasource is gwas (best score 0.8 across rows).
        let gwas = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((gwas.value(0) - 0.8).abs() < 1e-12);
        assert!((gwas.value(1) - 0.4).abs() < 1e-12);
        // Novelty null survives.
        let novelty = batch
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(!novelty.is_null(0) && novelty.is_null(1));
    }

    #[test]
    fn target_variant_carries_extra_identity_columns() {
        let ds = vec![component("uniprot", 0.6)];
        let rows = vec![row(
            "ENSG1",
            "BRCA1",
            ["Breast cancer 1", "protein_coding"],
            0.77,
            None,
            &ds,
            &[],
        )];
        let batch = build_breakdown_batch(rows.into_iter(), 3, 3, "target_id", "symbol").unwrap();
        // target_id, symbol, score, novelty, approved_name, biotype, ds_uniprot
        assert_eq!(batch.num_columns(), 7);
        let name = batch
            .column(4)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(name.value(0), "Breast cancer 1");
    }

    #[test]
    fn zero_top_keeps_only_base_columns() {
        let ds = vec![component("x", 0.2)];
        let rows = vec![row("E", "n", ["", ""], 0.1, None, &ds, &[])];
        let batch =
            build_breakdown_batch(rows.into_iter(), 0, 0, "disease_id", "disease_name").unwrap();
        assert_eq!(batch.num_columns(), 4);
    }
}

#[cfg(test)]
mod genetic_tests {
    use super::*;

    use opentargets::{CredibleSetStudy, CredibleSetVariant, L2GPrediction, L2GTarget};

    fn locus(id: &str, l2g: Vec<L2GPrediction>) -> CredibleSetLocus {
        CredibleSetLocus {
            study_locus_id: id.into(),
            study_id: "GCST90018961".into(),
            study_type: Some("gwas".into()),
            purity_mean_r2: None,
            p_value_mantissa: Some(5.204),
            p_value_exponent: Some(-21),
            beta: Some(0.0654),
            study: Some(CredibleSetStudy {
                trait_from_source: Some("LDL cholesterol".into()),
                ..Default::default()
            }),
            variant: Some(CredibleSetVariant {
                id: Some("17_66232877_G_C".into()),
                rs_ids: Some(vec!["rs149394327".into()]),
            }),
            l2g_predictions: opentargets::L2GPredictionPage {
                count: l2g.len() as i64,
                rows: l2g,
            },
        }
    }

    fn prediction(gene: &str, symbol: &str, score: f64) -> L2GPrediction {
        L2GPrediction {
            score,
            target: L2GTarget {
                id: gene.into(),
                approved_symbol: Some(symbol.into()),
            },
        }
    }

    #[test]
    fn genetics_long_format_crosses_loci_with_genes() {
        let loci = vec![
            (
                "1_1_A_T".into(),
                locus(
                    "locus1",
                    vec![
                        prediction("ENSG1", "APOH", 0.99),
                        prediction("ENSG2", "PRKCA", 0.05),
                    ],
                ),
            ),
            ("2_2_C_G".into(), locus("locus2", vec![])),
        ];
        let batch = build_genetic_batch(loci, 0.0).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 13);
        let symbol = batch
            .column(11)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(symbol.value(0), "APOH");
        assert_eq!(symbol.value(1), "PRKCA");
        // pval reassembled from mantissa × 10^exponent.
        let pval = batch
            .column(7)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((pval.value(0) - 5.204e-21).abs() < 1e-30);
        // Locus without predicted genes is dropped entirely.
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids.value(1), "1_1_A_T"); // only locus1 rows survive
    }

    #[test]
    fn genetics_min_l2g_filters_gene_rows() {
        let loci = vec![(
            "v".into(),
            locus(
                "locus1",
                vec![
                    prediction("ENSG1", "APOH", 0.9),
                    prediction("ENSG2", "PRKCA", 0.2),
                ],
            ),
        )];
        let batch = build_genetic_batch(loci, 0.5).unwrap();
        assert_eq!(batch.num_rows(), 1);
        let genes = batch
            .column(10)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(genes.value(0), "ENSG1");
    }

    #[test]
    fn genetics_empty_input_yields_typed_empty_batch() {
        let batch = build_genetic_batch(Vec::new(), 0.0).unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 13);
        // No NullArray: every column keeps its declared type.
        for column in batch.columns() {
            assert_ne!(column.data_type(), &arrow_schema::DataType::Null);
        }
    }

    /// Upstream `core/sql` with `concat(...)` emits `variant_id` as
    /// `Utf8View`; the node must accept it exactly like the `Utf8` column a
    /// TSV round-trip through `file_to_dataframe` produces.
    #[tokio::test]
    async fn genetics_id_column_accepts_utf8_view() {
        use arrow_array::StringViewArray;
        use arrow_schema::{DataType, Field, Schema};
        use datafusion::prelude::SessionContext;
        use std::sync::Arc;

        let ids = vec![
            Some("1_555168_A_G".to_string()),
            None,
            Some("17_66232877_G_C_longer_than_12_bytes".to_string()),
            Some("1_555168_A_G".to_string()), // duplicate collapses
        ];
        let frame = |layout: DataType| {
            let ctx = SessionContext::new();
            let schema = Arc::new(Schema::new(vec![Field::new("variant_id", layout.clone(), true)]));
            let values: Vec<Option<&str>> =
                ids.iter().map(|id| id.as_deref()).collect();
            let column: arrow_array::ArrayRef = match layout {
                DataType::Utf8 => Arc::new(StringArray::from(values.clone())),
                _ => Arc::new(StringViewArray::from(values)),
            };
            let batch = RecordBatch::try_new(schema, vec![column]).unwrap();
            ctx.read_batch(batch).unwrap()
        };

        let from_view = genetic_string_values(&frame(DataType::Utf8View), "variant_id")
            .await
            .unwrap();
        let from_utf8 = genetic_string_values(&frame(DataType::Utf8), "variant_id")
            .await
            .unwrap();
        // Null dropped, duplicate deduped in first-seen order — identical
        // request IDs regardless of the string layout.
        assert_eq!(from_view, vec!["1_555168_A_G", "17_66232877_G_C_longer_than_12_bytes"]);
        assert_eq!(from_view, from_utf8);
    }
}

/// Query credible-set study associations without requiring L2G predictions.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpentargetsPhewasSpec {
    /// Column containing mapped GRCh38 `chr_pos_ref_alt` IDs.
    #[serde(default = "default_genetic_id_column")]
    pub id_column: String,
    /// GraphQL endpoint override.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Maximum pages per variant (default 1000); truncation is reported.
    #[serde(default)]
    pub max_pages: Option<u32>,
}

#[derive(Clone)]
pub struct OpentargetsPhewasNode {
    meta: NodePorts,
    spec: OpentargetsPhewasSpec,
}
pub struct OpentargetsPhewasNodeFactory;

impl NodeFactory for OpentargetsPhewasNodeFactory {
    fn kind(&self) -> &'static str {
        "opentargets_phewas"
    }
    fn desc(&self) -> &'static str {
        "Auditable variant–study credible-set associations without L2G filtering."
    }
    fn doc(&self) -> &'static str {
        "Input: a DataFrame (e.g. from a file source) with GRCh38 chr_pos_ref_alt IDs. rsIDs must first be mapped and allele-checked externally. Output 0 contains one row per returned study locus, including query ID, lead ID, relationship, study trait, p-value components and discovery/replication sample ancestry and LD population structure as JSON. Output 1 audits each unique input ID, including invalid IDs, errors, page counts, reported counts and truncation. Credible-set membership is not necessarily a direct association; this API does not establish complete PheWAS coverage or equivalence to a historical BigQuery dataset. Study ancestry metadata must be interpreted before filtering."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpentargetsPhewasSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port_of_type_with_label(None, PortType::DataFrame, "ids")
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(OpentargetsPhewasNode {
            meta: self.ports(),
            spec: serde_json::from_value(spec)?,
        }))
    }
}

const PHEWAS_QUERY: &str = r#"query($ids: [String!]!, $page: Pagination!) {
  credibleSets(variantIds: $ids, page: $page) { count rows {
    studyLocusId studyId studyType pValueMantissa pValueExponent beta
    variant { id rsIds }
    study { traitFromSource initialSampleSize discoverySamples { ancestry sampleSize }
      replicationSamples { ancestry sampleSize } ldPopulationStructure { ldPopulation relativeSampleSize } }
  } }
}"#;

fn valid_ot_variant(id: &str) -> bool {
    let parts: Vec<_> = id.split('_').collect();
    parts.len() == 4
        && !parts[0].is_empty()
        && parts[1].parse::<u64>().is_ok_and(|n| n > 0)
        && parts[2..]
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| matches!(b, b'A' | b'C' | b'G' | b'T')))
}

// JSON string columns preserve nulls and the exact nested ancestry metadata.
fn phewas_batch(rows: &[Vec<Option<String>>], names: &[&str]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(
        names.iter().map(|n| field(n, true)).collect::<Vec<_>>(),
    ));
    let columns = (0..names.len())
        .map(|i| str_array(rows.iter().map(|r| r[i].clone()).collect()))
        .collect();
    RecordBatch::try_new(schema, columns)
        .map_err(|e| DagError::Schedule(format!("Open Targets output: {e}")))
}
fn json_value(v: &serde_json::Value) -> Option<String> {
    if v.is_null() {
        None
    } else if let Some(s) = v.as_str() {
        Some(s.to_owned())
    } else {
        Some(v.to_string())
    }
}

#[async_trait]
impl DagNode for OpentargetsPhewasNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "opentargets_phewas"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let df = inputs
            .first()
            .ok_or_else(|| {
                DagError::Schedule("opentargets_phewas requires an input DataFrame".into())
            })?
            .dataframe()
            .map_err(|e| DagError::Schedule(format!("input is not a DataFrame: {e}")))?;
        let ids = genetic_string_values(&df, &self.spec.id_column).await?;
        let column_index = df
            .schema()
            .index_of_column_by_name(None, &self.spec.id_column)
            .ok_or_else(|| DagError::Schedule("missing ID column".into()))?;
        let input_batches = df
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::Schedule(format!("failed to collect input: {e}")))?;
        let null_count: usize = input_batches
            .iter()
            .map(|batch| batch.column(column_index).null_count())
            .sum();
        let client = client_from_endpoint(&self.spec.endpoint);
        let mut associations = Vec::new();
        let mut audit = Vec::new();
        if null_count > 0 {
            audit.push(vec![
                None,
                Some("unresolved_id".into()),
                None,
                Some("0".into()),
                Some("0".into()),
                Some(format!("{null_count} null input rows")),
            ]);
        }
        let limit = self.spec.max_pages.unwrap_or(1000).max(1);
        for id in ids {
            let mut status = "complete".to_string();
            let mut error = None;
            let mut total: Option<i64> = None;
            let mut fetched = 0usize;
            let mut pages = 0u32;
            if !valid_ot_variant(&id) {
                status = "unresolved_id".into();
            } else {
                for index in 0..limit {
                    let response: Result<serde_json::Value, _> = client
                        .query(
                            PHEWAS_QUERY,
                            serde_json::json!({"ids":[id], "page":{"index":index,"size":100}}),
                        )
                        .await;
                    let data = match response {
                        Ok(d) => d,
                        Err(e) => {
                            status = "failed".into();
                            error = Some(e.to_string());
                            break;
                        }
                    };
                    let page = &data["credibleSets"];
                    let (Some(count), Some(rows)) =
                        (page["count"].as_i64(), page["rows"].as_array())
                    else {
                        status = "failed".into();
                        error = Some("invalid credibleSets response".into());
                        break;
                    };
                    pages += 1;
                    if total.is_some_and(|n| n != count) {
                        status = "inconsistent_count".into();
                        error = Some("reported count changed during pagination".into());
                        break;
                    }
                    total = Some(count);
                    fetched += rows.len();
                    for row in rows {
                        let lead = row["variant"]["id"].as_str();
                        let relation = match lead {
                            Some(v) if v == id => "lead",
                            Some(_) => "non_lead",
                            None => "unknown",
                        };
                        associations.push(vec![
                            Some(id.clone()),
                            json_value(&row["studyLocusId"]),
                            json_value(&row["studyId"]),
                            json_value(&row["studyType"]),
                            json_value(&row["study"]["traitFromSource"]),
                            json_value(&row["variant"]["id"]),
                            json_value(&row["variant"]["rsIds"]),
                            Some(relation.into()),
                            json_value(&row["pValueMantissa"]),
                            json_value(&row["pValueExponent"]),
                            json_value(&row["beta"]),
                            json_value(&row["study"]["initialSampleSize"]),
                            json_value(&row["study"]["discoverySamples"]),
                            json_value(&row["study"]["replicationSamples"]),
                            json_value(&row["study"]["ldPopulationStructure"]),
                        ]);
                    }
                    if fetched as i64 >= count {
                        if count == 0 {
                            status = "no_results".into();
                        }
                        break;
                    }
                    if rows.is_empty() {
                        status = "incomplete".into();
                        error = Some("empty page before reported count".into());
                        break;
                    }
                    if index + 1 == limit {
                        status = "truncated".into();
                    }
                }
            }
            audit.push(vec![
                Some(id),
                Some(status),
                total.map(|n| n.to_string()),
                Some(fetched.to_string()),
                Some(pages.to_string()),
                error,
            ]);
        }
        let result = phewas_batch(
            &associations,
            &[
                "input_id",
                "study_locus_id",
                "study_id",
                "study_type",
                "trait",
                "lead_variant_id",
                "lead_variant_rsids",
                "relationship",
                "pvalue_mantissa",
                "pvalue_exponent",
                "beta",
                "initial_sample_size",
                "discovery_samples_json",
                "replication_samples_json",
                "ld_population_structure_json",
            ],
        )?;
        let audit = phewas_batch(
            &audit,
            &[
                "input_id",
                "status",
                "reported_count",
                "returned_count",
                "pages_fetched",
                "error",
            ],
        )?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, batch_to_df(&ctx.session(), result)?);
        outputs.insert(1, batch_to_df(&ctx.session(), audit)?);
        Ok(outputs)
    }
}

#[cfg(test)]
mod phewas_tests {
    use super::*;

    #[test]
    fn validates_variant_format_without_claiming_mapping() {
        assert!(valid_ot_variant("17_66232877_G_C"));
        assert!(valid_ot_variant("1_123_AT_A"));
        assert!(!valid_ot_variant("rs123"));
        assert!(!valid_ot_variant("1_0_A_C"));
        assert!(!valid_ot_variant("1_123_A_N"));
    }

    #[test]
    fn empty_outputs_keep_schema_and_nested_json_is_preserved() {
        let batch = phewas_batch(&[], &["input_id", "status"]).unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 2);
        let ancestry = serde_json::json!([{ "ancestry": "European", "sampleSize": 42 }]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json_value(&ancestry).unwrap()).unwrap(),
            ancestry
        );
        assert_eq!(json_value(&serde_json::Value::Null), None);
    }
}
