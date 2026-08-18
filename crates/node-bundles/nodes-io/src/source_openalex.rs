//! OpenAlex source nodes.
//!
//! Two source nodes that pull tabular data from the [OpenAlex API](
//! https://api.openalex.org) and emit it as a DataFusion `DataFrame`, so
//! structured bibliometric data can flow through a DAG (aggregate, join,
//! sink to CSV/VFS, …).
//!
//! - [`OpenAlexWorksNode`] (`source_openalex_works`) — fetch works as a
//!   flat table (id, doi, title, year, type, cited_by_count, oa_status,
//!   journal, …). Supports filter, search, sort, cursor auto-paging.
//! - [`OpenAlexGroupByNode`] (`source_openalex_group_by`) — aggregate works
//!   by a field (e.g. `publication_year`, `type`, `open_access.oa_status`)
//!   and return a two-column count table.
//!
//! Both are zero-input / single-output source nodes. They reuse the
//! `openalex` SDK client; `DagNode::execute` is async on the engine's
//! tokio runtime, so the client is simply `.await`ed.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, RecordBatch, StringArray, UInt16Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use openalex::{ListParams, OpenAlexClient, Work};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build an [`OpenAlexClient`] using the API key from the `OPENALEX_API_KEY`
/// environment variable (if set and non-empty).
///
/// OpenAlex's API is free; the optional premium key only raises the daily
/// rate-limit budget, so a missing/unset variable falls back to the
/// anonymous tier rather than erroring.
fn client_from_env() -> OpenAlexClient {
    let key = std::env::var("OPENALEX_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    OpenAlexClient::new(key.as_deref())
}

/// Turn a `RecordBatch` into a `DataFrame` via a fresh isolated session.
fn batch_to_df(ctx: &SessionContext, batch: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read OpenAlex batch: {e}")))
}

fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}

fn opt_str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    str_array(rows)
}

fn u64_array(rows: Vec<Option<u64>>) -> Arc<dyn Array> {
    Arc::new(UInt64Array::from(rows))
}

// ===========================================================================
// 1. Works table node
// ===========================================================================

/// Spec for [`OpenAlexWorksNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpenAlexWorksSpec {
    /// Raw OpenAlex filter expression (field:value pairs joined by commas).
    /// Example: `"publication_year:2024,is_oa:true,cited_by_count:>100"`.
    #[serde(default)]
    pub filter: Option<String>,
    /// Full-text search query.
    #[serde(default)]
    pub search: Option<String>,
    /// Sort expression, e.g. `"cited_by_count:desc"`.
    #[serde(default)]
    pub sort: Option<String>,
    /// Auto-paginate via cursor to fetch **all** matching works (capped at
    /// `max_rows`). Default `false` — returns one page only.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Page size (1–100, default 25). Used for each request; with
    /// `fetch_all=true` this is the per-request batch size.
    #[serde(default)]
    pub per_page: Option<u32>,
    /// Hard cap on total rows when `fetch_all=true`. Default 10 000.
    #[serde(default)]
    pub max_rows: Option<usize>,
    /// Comma-separated list of fields to request (reduces payload size).
    /// Does NOT affect output columns — those are always the schema below.
    #[serde(default)]
    pub select: Option<String>,
}

/// Source node emitting a works table from OpenAlex.
#[derive(Clone)]
pub struct OpenAlexWorksNode {
    meta: NodePorts,
    spec: OpenAlexWorksSpec,
}

pub struct OpenAlexWorksNodeFactory {}

fn works_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for OpenAlexWorksNodeFactory {
    fn kind(&self) -> &'static str {
        "source_openalex_works"
    }

    fn desc(&self) -> &'static str {
        "Fetch scholarly works from OpenAlex as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the OpenAlex /works endpoint and emits the results \
        as a DataFrame. Supports filter, search, sort, and optional cursor auto-paging.\n\n\
        Output schema: \
        `id, doi, title, publication_year, type, language, cited_by_count, is_oa, \
        oa_status, journal, first_author, author_count, is_retracted`.\n\n\
        Set `fetch_all=true` to retrieve all matching works across pages (capped by \
        `max_rows`). Use `filter` to narrow results (e.g. \
        `publication_year:2024,authorships.institutions.id:I27837315`)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpenAlexWorksSpec)
    }

    fn ports(&self) -> NodePorts {
        works_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpenAlexWorksSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpenAlexWorksNode {
            meta: works_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpenAlexWorksNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_openalex_works"
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
        let client = client_from_env();
        let max_rows = self.spec.max_rows.unwrap_or(10_000);

        let works: Vec<Work> = if self.spec.fetch_all.unwrap_or(false) {
            fetch_works_all(&client, &self.spec, max_rows).await?
        } else {
            fetch_works_page(&client, &self.spec).await?
        };

        let session = ctx.session();
        let batch = build_works_batch(works)?;
        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

async fn fetch_works_page(
    client: &OpenAlexClient,
    spec: &OpenAlexWorksSpec,
) -> Result<Vec<Work>, DagError> {
    let mut params = ListParams::new();
    if let Some(ref f) = spec.filter {
        params = params.with_filter(f);
    }
    if let Some(ref s) = spec.search {
        params = params.with_search(s);
    }
    if let Some(ref s) = spec.sort {
        params = params.with_sort(s);
    }
    if let Some(pp) = spec.per_page {
        params = params.with_per_page(pp.min(100));
    }
    if let Some(ref sel) = spec.select {
        params = params.with_select(sel);
    }
    client
        .list_works(&params)
        .await
        .map(|r| r.results)
        .map_err(|e| DagError::Schedule(format!("OpenAlex request failed: {e}")))
}

async fn fetch_works_all(
    client: &OpenAlexClient,
    spec: &OpenAlexWorksSpec,
    max_rows: usize,
) -> Result<Vec<Work>, DagError> {
    let mut params = ListParams::new();
    if let Some(ref f) = spec.filter {
        params = params.with_filter(f);
    }
    if let Some(ref s) = spec.search {
        params = params.with_search(s);
    }
    if let Some(ref s) = spec.sort {
        params = params.with_sort(s);
    }
    params.per_page = Some(spec.per_page.unwrap_or(100).min(100));
    // Select isn't passed to list_works_all since it may omit required fields.

    let mut all = Vec::new();
    let mut cursor_params = params.clone();
    cursor_params.cursor = Some("*".to_string());

    loop {
        let resp = client
            .list_works(&cursor_params)
            .await
            .map_err(|e| DagError::Schedule(format!("OpenAlex paging failed: {e}")))?;

        let n = resp.results.len();
        all.extend(resp.results);
        if all.len() >= max_rows {
            all.truncate(max_rows);
            break;
        }

        match resp.meta.next_cursor {
            Some(cursor) if !cursor.is_empty() && n > 0 => {
                cursor_params.cursor = Some(cursor);
            }
            _ => break,
        }
    }

    Ok(all)
}

/// Build the works batch with the output schema described in the node doc.
fn build_works_batch(works: Vec<Work>) -> Result<RecordBatch, DagError> {
    let ids: Vec<Option<String>> = works.iter().map(|w| Some(w.id.clone())).collect();
    let dois: Vec<Option<String>> = works.iter().map(|w| w.doi.clone()).collect();
    let titles: Vec<Option<String>> = works.iter().map(|w| w.title.clone()).collect();
    let years: Vec<Option<u16>> = works.iter().map(|w| w.publication_year).collect();
    let types: Vec<Option<String>> = works.iter().map(|w| w.type_.clone()).collect();
    let langs: Vec<Option<String>> = works.iter().map(|w| w.language.clone()).collect();
    let cited: Vec<u64> = works.iter().map(|w| w.cited_by_count).collect();
    let is_oa: Vec<bool> = works.iter().map(|w| w.open_access.is_oa).collect();
    let oa_status: Vec<Option<String>> = works
        .iter()
        .map(|w| w.open_access.oa_status.clone())
        .collect();
    let journals: Vec<Option<String>> = works
        .iter()
        .map(|w| {
            w.primary_location
                .as_ref()
                .and_then(|l| l.source.as_ref())
                .and_then(|s| s.display_name.clone())
        })
        .collect();
    let first_authors: Vec<Option<String>> = works
        .iter()
        .map(|w| {
            w.authorships
                .first()
                .and_then(|a| a.author.display_name.clone())
        })
        .collect();
    let author_counts: Vec<Option<u64>> = works
        .iter()
        .map(|w| Some(w.authorships.len() as u64))
        .collect();
    let is_retracted: Vec<bool> = works.iter().map(|w| w.is_retracted).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new("doi", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("publication_year", DataType::UInt16, true),
        Field::new("type", DataType::Utf8, true),
        Field::new("language", DataType::Utf8, true),
        Field::new("cited_by_count", DataType::UInt64, false),
        Field::new("is_oa", DataType::Boolean, false),
        Field::new("oa_status", DataType::Utf8, true),
        Field::new("journal", DataType::Utf8, true),
        Field::new("first_author", DataType::Utf8, true),
        Field::new("author_count", DataType::UInt64, true),
        Field::new("is_retracted", DataType::Boolean, false),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            opt_str_array(ids),
            opt_str_array(dois),
            opt_str_array(titles),
            Arc::new(UInt16Array::from(years)),
            opt_str_array(types),
            opt_str_array(langs),
            Arc::new(UInt64Array::from(cited)),
            Arc::new(BooleanArray::from(is_oa)),
            opt_str_array(oa_status),
            opt_str_array(journals),
            opt_str_array(first_authors),
            u64_array(author_counts),
            Arc::new(BooleanArray::from(is_retracted)),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build works batch: {e}")))
}

// ===========================================================================
// 2. Group-by aggregation node
// ===========================================================================

/// Spec for [`OpenAlexGroupByNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpenAlexGroupBySpec {
    /// Field to aggregate by, e.g. `"publication_year"`, `"type"`,
    /// `"open_access.oa_status"`, `"primary_location.source.id"`,
    /// `"authorships.institutions.id"`, `"topics.id"`.
    pub group_by: String,
    /// Optional filter to narrow the aggregation scope.
    #[serde(default)]
    pub filter: Option<String>,
    /// Optional search query.
    #[serde(default)]
    pub search: Option<String>,
}

/// Source node emitting an OpenAlex group-by aggregation table.
#[derive(Clone)]
pub struct OpenAlexGroupByNode {
    meta: NodePorts,
    spec: OpenAlexGroupBySpec,
}

pub struct OpenAlexGroupByNodeFactory {}

fn group_by_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for OpenAlexGroupByNodeFactory {
    fn kind(&self) -> &'static str {
        "source_openalex_group_by"
    }

    fn desc(&self) -> &'static str {
        "Aggregate OpenAlex works counts by a field, returning a count table."
    }

    fn doc(&self) -> &'static str {
        "A source node that runs a `group_by` aggregation on the OpenAlex /works \
        endpoint and emits the results as a two-column DataFrame: `key, count`.\n\n\
        Common `group_by` values: `publication_year`, `type`, `open_access.oa_status`, \
        `primary_location.source.id`, `authorships.institutions.id`, `topics.id`.\n\n\
        Combine with `filter` to scope the aggregation, e.g. count works by year for \
        a specific institution: \
        `{\"group_by\": \"publication_year\", \"filter\": \"authorships.institutions.id:I27837315\"}`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpenAlexGroupBySpec)
    }

    fn ports(&self) -> NodePorts {
        group_by_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpenAlexGroupBySpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpenAlexGroupByNode {
            meta: group_by_port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpenAlexGroupByNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_openalex_group_by"
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
        let client = client_from_env();
        let mut params = ListParams::new().with_group_by(&self.spec.group_by);
        if let Some(ref f) = self.spec.filter {
            params = params.with_filter(f);
        }
        if let Some(ref s) = self.spec.search {
            params = params.with_search(s);
        }

        let resp = client
            .list_works(&params)
            .await
            .map_err(|e| DagError::Schedule(format!("OpenAlex group_by failed: {e}")))?;

        let entries = resp.group_by;

        // Build a two-column batch: key, count.
        let keys: Vec<Option<String>> = entries
            .iter()
            .map(|e| {
                e.key_display_name
                    .clone()
                    .or_else(|| e.key_name.clone())
                    .unwrap_or_else(|| e.key.clone())
            })
            .map(Some)
            .collect();
        let key_ids: Vec<Option<String>> = entries.iter().map(|e| Some(e.key.clone())).collect();
        let counts: Vec<u64> = entries.iter().map(|e| e.count).collect();

        let session = ctx.session();
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("key_id", DataType::Utf8, true),
            Field::new("count", DataType::UInt64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                opt_str_array(keys),
                opt_str_array(key_ids),
                Arc::new(UInt64Array::from(counts)),
            ],
        )
        .map_err(|e| DagError::Schedule(format!("failed to build group_by batch: {e}")))?;

        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
