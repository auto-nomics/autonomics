//! Semantic Scholar source nodes.
//!
//! Two source nodes that pull tabular data from the [Semantic Scholar Academic
//! Graph API](https://api.semanticscholar.org/graph/v1) and emit it as a
//! DataFusion `DataFrame`, so paper metadata tables and author search hits
//! can flow through a DAG (join, filter via `sql_node`, sink to
//! CSV/VFS, …).
//!
//! - [`S2PaperSearchNode`] (`source_s2_paper_search`) — relevance paper
//!   search → structured table.
//! - [`S2AuthorSearchNode`] (`source_s2_author_search`) — author search
//!   → structured table.
//!
//! Both are zero-input / single-output source nodes. They reuse the
//! `semantic_scholar` SDK client; `DagNode::execute` is async on the
//! engine's tokio runtime.

use std::sync::Arc;

use arrow_array::{Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use semantic_scholar::{PaperSearchFilter, S2Client};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build an [`S2Client`] from an optional API key.
fn client_from_api_key(api_key: &Option<String>) -> S2Client {
    match api_key {
        Some(key) => S2Client::new().with_api_key(key),
        None => S2Client::new(),
    }
}

/// Turn a `RecordBatch` into a `DataFrame`.
fn batch_to_df(ctx: &SessionContext, batch: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read S2 batch: {e}")))
}

fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}

fn i32_array(rows: Vec<Option<i32>>) -> Arc<dyn Array> {
    Arc::new(Int32Array::from(rows))
}

// ===========================================================================
// 1. Paper search node
// ===========================================================================

/// Spec for [`S2PaperSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct S2PaperSearchSpec {
    /// Plain-text search query matched against title and abstract.
    pub query: String,
    /// Maximum number of results (default 50, max 100 for relevance search).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Year range filter, e.g. `"2020-2023"`, `"2019"`, `"2010-"`.
    #[serde(default)]
    pub year: Option<String>,
    /// Venue filter (comma-separated for OR).
    #[serde(default)]
    pub venue: Option<String>,
    /// Fields of study filter (comma-separated for OR).
    #[serde(default)]
    pub fields_of_study: Option<String>,
    /// Publication types filter (comma-separated for OR).
    #[serde(default)]
    pub publication_types: Option<String>,
    /// Only return papers with open access PDF. Default false.
    #[serde(default)]
    pub open_access_pdf: Option<bool>,
    /// Minimum citation count filter.
    #[serde(default)]
    pub min_citation_count: Option<i32>,
    /// Semantic Scholar API key for higher rate limits.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Source node emitting a Semantic Scholar paper search table.
#[derive(Clone)]
pub struct S2PaperSearchNode {
    meta: NodePorts,
    spec: S2PaperSearchSpec,
}

pub struct S2PaperSearchNodeFactory {}

fn paper_search_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for S2PaperSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_s2_paper_search"
    }

    fn desc(&self) -> &'static str {
        "Search Semantic Scholar papers and emit results as a structured table."
    }

    fn doc(&self) -> &'static str {
        "A source node that runs a paper relevance search on the Semantic Scholar \
        Academic Graph API and emits the results as a DataFrame. \
        No input ports; one output port.\n\n\
        Output schema: \
        `paper_id, title, abstract, year, venue, doi, arxiv_id, pubmed_id, \
        authors, citation_count, reference_count, influential_citation_count, \
        publication_date, is_open_access, open_access_pdf, tldr`.\n\n\
        Use filters (`year`, `venue`, `fields_of_study`, `publication_types`, \
        `open_access_pdf`, `min_citation_count`) to constrain the result, \
        then pipe into `sql_node` or `sink_file`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(S2PaperSearchSpec)
    }

    fn ports(&self) -> NodePorts {
        paper_search_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: S2PaperSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(S2PaperSearchNode {
            meta: paper_search_port_layout(),
            spec: node_spec,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<S2PaperSearchSpec>(spec, "source_s2_paper_search")?;
        let out = ctx.output_var.to_string();
        let code = vec![
            format!("# Semantic Scholar: paper search for '{}'", s.query),
            format!("# NOTE: Uses the S2 Academic Graph REST API via httr"),
            format!("{out} <- httr::content(httr::GET("),
            format!("  \"https://api.semanticscholar.org/graph/v1/paper/search\",",),
            format!(
                "  query = list(query = \"{}\", limit = {}, fields = \"paperId,title,year,venue,citationCount,referenceCount,isOpenAccess,openAccessPdf,externalIds,tldr\"),",
                s.query,
                s.limit.unwrap_or(50)
            ),
            format!("  add_headers(`x-api-key` = Sys.getenv(\"S2_API_KEY\"))"),
            format!("))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["httr".into()]
    }
}

#[async_trait]
impl DagNode for S2PaperSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_s2_paper_search"
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
        let client = client_from_api_key(&self.spec.api_key);
        let limit = self.spec.limit.unwrap_or(50);
        let filter = PaperSearchFilter {
            year: self.spec.year.clone(),
            venue: self.spec.venue.clone(),
            fields_of_study: self.spec.fields_of_study.clone(),
            publication_types: self.spec.publication_types.clone(),
            open_access_pdf: self.spec.open_access_pdf.unwrap_or(false),
            min_citation_count: self.spec.min_citation_count,
        };

        // Decide whether to use filtered or plain search.
        let has_filter = filter.year.is_some()
            || filter.venue.is_some()
            || filter.fields_of_study.is_some()
            || filter.publication_types.is_some()
            || filter.open_access_pdf
            || filter.min_citation_count.is_some();

        let resp = if has_filter {
            client
                .search_paper_filtered(&self.spec.query, limit, 0, &filter, None)
                .await
        } else {
            client.search_paper(&self.spec.query, limit, None).await
        }
        .map_err(|e| DagError::Schedule(format!("S2 search failed: {e}")))?;

        let session = ctx.session();
        let batch = build_paper_batch(resp.data)?;
        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the paper batch.
fn build_paper_batch(rows: Vec<semantic_scholar::Paper>) -> Result<RecordBatch, DagError> {
    let paper_ids: Vec<Option<String>> = rows.iter().map(|p| Some(p.paper_id.clone())).collect();
    let titles: Vec<Option<String>> = rows.iter().map(|p| p.title.clone()).collect();
    let abstracts: Vec<Option<String>> = rows.iter().map(|p| p.abstract_text.clone()).collect();
    let years: Vec<Option<i32>> = rows.iter().map(|p| p.year).collect();
    let venues: Vec<Option<String>> = rows.iter().map(|p| p.venue.clone()).collect();

    let dois: Vec<Option<String>> = rows
        .iter()
        .map(|p| p.external_ids.as_ref().and_then(|e| e.doi.clone()))
        .collect();
    let arxiv_ids: Vec<Option<String>> = rows
        .iter()
        .map(|p| p.external_ids.as_ref().and_then(|e| e.arxiv.clone()))
        .collect();
    let pubmed_ids: Vec<Option<String>> = rows
        .iter()
        .map(|p| p.external_ids.as_ref().and_then(|e| e.pubmed.clone()))
        .collect();

    let authors: Vec<Option<String>> = rows
        .iter()
        .map(|p| {
            let names: Vec<&str> = p.authors.iter().filter_map(|a| a.name.as_deref()).collect();
            if names.is_empty() {
                None
            } else {
                Some(names.join("; "))
            }
        })
        .collect();

    let citation_counts: Vec<Option<i32>> = rows.iter().map(|p| p.citation_count).collect();
    let reference_counts: Vec<Option<i32>> = rows.iter().map(|p| p.reference_count).collect();
    let influential_counts: Vec<Option<i32>> =
        rows.iter().map(|p| p.influential_citation_count).collect();
    let pub_dates: Vec<Option<String>> = rows.iter().map(|p| p.publication_date.clone()).collect();

    let is_oa: Vec<Option<String>> = rows
        .iter()
        .map(|p| {
            p.is_open_access
                .map(|b| if b { "true".into() } else { "false".into() })
        })
        .collect();

    let oa_pdfs: Vec<Option<String>> = rows
        .iter()
        .map(|p| p.open_access_pdf.as_ref().and_then(|o| o.url.clone()))
        .collect();

    let tldrs: Vec<Option<String>> = rows
        .iter()
        .map(|p| p.tldr.as_ref().and_then(|t| t.text.clone()))
        .collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("paper_id", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("abstract", DataType::Utf8, true),
        Field::new("year", DataType::Int32, true),
        Field::new("venue", DataType::Utf8, true),
        Field::new("doi", DataType::Utf8, true),
        Field::new("arxiv_id", DataType::Utf8, true),
        Field::new("pubmed_id", DataType::Utf8, true),
        Field::new("authors", DataType::Utf8, true),
        Field::new("citation_count", DataType::Int32, true),
        Field::new("reference_count", DataType::Int32, true),
        Field::new("influential_citation_count", DataType::Int32, true),
        Field::new("publication_date", DataType::Utf8, true),
        Field::new("is_open_access", DataType::Utf8, true),
        Field::new("open_access_pdf", DataType::Utf8, true),
        Field::new("tldr", DataType::Utf8, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            str_array(paper_ids),
            str_array(titles),
            str_array(abstracts),
            i32_array(years),
            str_array(venues),
            str_array(dois),
            str_array(arxiv_ids),
            str_array(pubmed_ids),
            str_array(authors),
            i32_array(citation_counts),
            i32_array(reference_counts),
            i32_array(influential_counts),
            str_array(pub_dates),
            str_array(is_oa),
            str_array(oa_pdfs),
            str_array(tldrs),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build S2 paper batch: {e}")))
}

// ===========================================================================
// 2. Author search node
// ===========================================================================

/// Spec for [`S2AuthorSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct S2AuthorSearchSpec {
    /// Author name to search for.
    pub query: String,
    /// Maximum number of results (default 50, max 1000).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Offset for pagination (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
    /// Semantic Scholar API key for higher rate limits.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Source node emitting a Semantic Scholar author search table.
#[derive(Clone)]
pub struct S2AuthorSearchNode {
    meta: NodePorts,
    spec: S2AuthorSearchSpec,
}

pub struct S2AuthorSearchNodeFactory {}

fn author_search_port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for S2AuthorSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_s2_author_search"
    }

    fn desc(&self) -> &'static str {
        "Search Semantic Scholar authors and emit results as a structured table."
    }

    fn doc(&self) -> &'static str {
        "A source node that runs an author name search on the Semantic Scholar \
        Academic Graph API and emits the results as a DataFrame. \
        No input ports; one output port.\n\n\
        Output schema: \
        `author_id, name, affiliations, homepage, paper_count, citation_count, h_index`.\n\n\
        Use this to resolve author names into stable S2 IDs inside a pipeline."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(S2AuthorSearchSpec)
    }

    fn ports(&self) -> NodePorts {
        author_search_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: S2AuthorSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(S2AuthorSearchNode {
            meta: author_search_port_layout(),
            spec: node_spec,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<S2AuthorSearchSpec>(spec, "source_s2_author_search")?;
        let out = ctx.output_var.to_string();
        let code = vec![
            format!("# Semantic Scholar: author search for '{}'", s.query),
            format!("{out} <- httr::content(httr::GET("),
            format!("  \"https://api.semanticscholar.org/graph/v1/author/search\",",),
            format!(
                "  query = list(query = \"{}\", limit = {}, fields = \"authorId,name,affiliations,homepage,paperCount,citationCount,hIndex\"),",
                s.query,
                s.limit.unwrap_or(50)
            ),
            format!("  add_headers(`x-api-key` = Sys.getenv(\"S2_API_KEY\"))"),
            format!("))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["httr".into()]
    }
}

#[async_trait]
impl DagNode for S2AuthorSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_s2_author_search"
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
        let client = client_from_api_key(&self.spec.api_key);
        let resp = client
            .search_authors(
                &self.spec.query,
                self.spec.limit.unwrap_or(50),
                self.spec.offset.unwrap_or(0),
                None,
            )
            .await
            .map_err(|e| DagError::Schedule(format!("S2 author search failed: {e}")))?;

        let session = ctx.session();
        let batch = build_author_batch(resp.data)?;
        let df = batch_to_df(&session, batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the author batch.
fn build_author_batch(rows: Vec<semantic_scholar::Author>) -> Result<RecordBatch, DagError> {
    let author_ids: Vec<Option<String>> = rows.iter().map(|a| Some(a.author_id.clone())).collect();
    let names: Vec<Option<String>> = rows.iter().map(|a| a.name.clone()).collect();
    let affiliations: Vec<Option<String>> = rows
        .iter()
        .map(|a| a.affiliations.as_ref().map(|v| v.join("; ")))
        .collect();
    let homepages: Vec<Option<String>> = rows.iter().map(|a| a.homepage.clone()).collect();
    let paper_counts: Vec<Option<i32>> = rows
        .iter()
        .map(|a| a.paper_count.map(|c| c as i32))
        .collect();
    let citation_counts: Vec<Option<i32>> = rows
        .iter()
        .map(|a| a.citation_count.map(|c| c as i32))
        .collect();
    let h_indices: Vec<Option<i32>> = rows.iter().map(|a| a.h_index.map(|c| c as i32)).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("author_id", DataType::Utf8, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("affiliations", DataType::Utf8, true),
        Field::new("homepage", DataType::Utf8, true),
        Field::new("paper_count", DataType::Int32, true),
        Field::new("citation_count", DataType::Int32, true),
        Field::new("h_index", DataType::Int32, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            str_array(author_ids),
            str_array(names),
            str_array(affiliations),
            str_array(homepages),
            i32_array(paper_counts),
            i32_array(citation_counts),
            i32_array(h_indices),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build S2 author batch: {e}")))
}
