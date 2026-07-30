//! Open Targets Platform source nodes.
//!
//! Two source nodes that pull tabular data from the [Open Targets Platform](
//! https://platform.opentargets.org/api) GraphQL API and emit it as a
//! DataFusion `DataFrame`, so association score tables and search hits can
//! flow through a DAG (join, filter via `sql_node`, sink to CSV/Iceberg, …).
//!
//! - [`OpentargetsAssociationsNode`] (`source_opentargets_associations`) —
//!   target↔disease association score table.
//! - [`OpentargetsSearchNode`] (`source_opentargets_search`) — full-text
//!   search hits table (resolve names → stable IDs inside a pipeline).
//!
//! Both are zero-input / single-output source nodes. They reuse the
//! `opentargets` SDK client; `DagNode::execute` is async on the engine's
//! tokio runtime, so the client is simply `.await`ed.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use opentargets::{
    AssociatedDisease, AssociatedTarget, OpenTargetsClient, Pagination, SearchResult,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

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
        `sql_node` or `sink_file`."
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
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
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
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
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
                let rows = assoc_disease_rows(&client, &self.spec, enable_indirect, b_filter)
                    .await?;
                build_disease_batch(&session, &self.spec.id, rows, min_score)?
            }
            "disease_to_target" => {
                let rows = assoc_target_rows(&client, &self.spec, enable_indirect, b_filter)
                    .await?;
                build_target_batch(&session, &self.spec.id, rows, min_score)?
            }
            other => {
                return Err(DagError::Schedule(format!(
                    "unknown direction '{other}' (expected 'target_to_disease' or 'disease_to_target')"
                )));
            }
        };

        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = HashMap::new();
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
    let disease_ids: Vec<Option<String>> = rows.iter().map(|a| Some(a.disease.id.clone())).collect();
    let disease_names: Vec<Option<String>> = rows.iter().map(|a| Some(a.disease.name.clone())).collect();
    let scores: Vec<Option<f64>> = rows.iter().map(|a| Some(a.score)).collect();
    let novelties: Vec<Option<f64>> = rows.iter().map(|a| a.novelty).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("target_id", DataType::Utf8, true),
        Field::new("disease_id", DataType::Utf8, true),
        Field::new("disease_name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("novelty", DataType::Float64, true),
    ]));
    RecordBatch::try_new(schema, vec![
        str_array(target_ids),
        str_array(disease_ids),
        str_array(disease_names),
        f64_array(scores),
        f64_array(novelties),
    ])
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
    let symbols: Vec<Option<String>> = rows.iter().map(|a| Some(a.target.approved_symbol.clone())).collect();
    let names: Vec<Option<String>> = rows.iter().map(|a| Some(a.target.approved_name.clone())).collect();
    let biotypes: Vec<Option<String>> = rows.iter().map(|a| Some(a.target.biotype.clone())).collect();
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
    RecordBatch::try_new(schema, vec![
        str_array(disease_ids),
        str_array(target_ids),
        str_array(symbols),
        str_array(names),
        str_array(biotypes),
        f64_array(scores),
        f64_array(novelties),
    ])
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
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
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
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
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
        let mut res: PortOutputs = HashMap::new();
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
    RecordBatch::try_new(schema, vec![
        str_array(entities),
        str_array(ids),
        str_array(names),
        f64_array(scores),
        str_array(descriptions),
    ])
    .map_err(|e| DagError::Schedule(format!("failed to build search batch: {e}")))
}
