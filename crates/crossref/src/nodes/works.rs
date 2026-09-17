//! `source_crossref_works` — search Crossref `/works` and emit a DataFrame.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::client::{CrossrefClient, WorksQuery};
use crate::types::Work;

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// Spec for [`CrossrefWorksNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct CrossrefWorksSpec {
    /// Free-text search query for `/works`. Required if no filters are
    /// specified.
    #[serde(default)]
    pub query: Option<String>,

    /// Crossref filters as `name:value` pairs (e.g.
    /// `{ "type": "journal-article", "has-abstract": "true" }`).
    ///
    /// Multiple filters are AND-ed. For the full list see
    /// <https://www.crossref.org/documentation/retrieve-metadata/rest-api/>.
    #[serde(default)]
    pub filters: Option<Vec<Vec<String>>>,

    /// Sort field: `relevance`, `published`, `deposited`, `indexed`,
    /// `is-referenced-by-count`, `references-count`. Default: `relevance`.
    #[serde(default)]
    pub sort: Option<String>,

    /// Sort order: `asc` or `desc`. Default: `desc`.
    #[serde(default)]
    pub order: Option<String>,

    /// Number of rows per page (max 1000). Default: 100.
    #[serde(default)]
    pub rows: Option<u32>,

    /// Maximum total number of works to fetch across pages. Default: same as
    /// `rows` (single page).
    #[serde(default)]
    pub max_results: Option<u32>,

    /// Contact email for the Crossref polite pool (recommended).
    #[serde(default)]
    pub mailto: Option<String>,

    /// Crossref Plus bearer token.
    #[serde(default)]
    pub bearer_token: Option<String>,

    /// Select specific fields to return from the API (e.g.
    /// `["DOI","title","author"]`). Improves performance by reducing payload
    /// size. When omitted, all fields are fetched.
    #[serde(default)]
    pub select: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Node + Factory
// ---------------------------------------------------------------------------

/// Source node emitting a Crossref works table.
#[derive(Clone)]
pub struct CrossrefWorksNode {
    meta: NodePorts,
    spec: CrossrefWorksSpec,
}

pub struct CrossrefWorksNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for CrossrefWorksNodeFactory {
    fn kind(&self) -> &'static str {
        "source_crossref_works"
    }

    fn desc(&self) -> &'static str {
        "Search Crossref /works and emit scholarly metadata as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the Crossref REST API `/works` endpoint and \
        emits the results as a DataFrame. No input ports; one output port.\n\n\
        Use `query` for free-text search, and/or `filters` for structured filters \
        (type, has-abstract, from-pub-date, etc.).\n\n\
        Output schema: `doi, title, type, container_title, publisher, year, month, \
        volume, issue, page, issn, authors, cited_by_count, references_count, \
        abstract`.\n\n\
        Set `max_results` > `rows` to auto-paginate via cursor. Pipe into \
        `sql_node` or `dataframe_to_file` for downstream processing."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CrossrefWorksSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: CrossrefWorksSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CrossrefWorksNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for CrossrefWorksNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_crossref_works"
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
        // Build client.
        let mut builder = CrossrefClient::builder();
        if let Some(ref email) = self.spec.mailto {
            builder = builder.mailto(email);
        }
        if let Some(ref token) = self.spec.bearer_token {
            builder = builder.bearer_token(token);
        }
        let client = builder.build();

        // Build query.
        let rows_per_page = self.spec.rows.unwrap_or(100).min(1000);
        let max_results = self.spec.max_results.unwrap_or(rows_per_page);

        let mut query = WorksQuery::new().with_rows(rows_per_page);
        if let Some(ref q) = self.spec.query {
            query = query.with_query(q);
        }
        if let Some(ref filters) = self.spec.filters {
            for pair in filters {
                if pair.len() == 2 {
                    query = query.with_filter(&pair[0], &pair[1]);
                }
            }
        }
        if let Some(ref s) = self.spec.sort {
            query = query.with_sort(s);
        }
        if let Some(ref o) = self.spec.order {
            query = query.with_order(o);
        }
        if let Some(ref fields) = self.spec.select {
            query = query.with_select(fields.clone());
        }

        // Fetch with auto-pagination if needed.
        let mut all_works: Vec<Work> = Vec::new();
        let mut cursor: Option<String> = None;
        let pages = if rows_per_page > 0 {
            max_results.div_ceil(rows_per_page)
        } else {
            1
        };

        for _page in 0..pages {
            if all_works.len() >= max_results as usize {
                break;
            }
            let mut page_query = query.clone();
            match &cursor {
                None => {
                    // First page: use cursor="*" if we plan to paginate.
                    if pages > 1 {
                        page_query = page_query.with_cursor("*");
                    }
                }
                Some(c) => {
                    page_query = page_query.with_cursor(c);
                }
            }

            let resp = client
                .works(&page_query)
                .await
                .map_err(|e| DagError::Schedule(format!("Crossref request failed: {e}")))?;

            let msg = &resp.message;
            if msg.items.is_empty() {
                break;
            }
            all_works.extend(msg.items.iter().cloned());

            // Update cursor for next page.
            cursor = msg.next_cursor.clone();
            if cursor.is_none() {
                break;
            }
        }

        // Truncate to max_results.
        all_works.truncate(max_results as usize);

        // Build DataFrame.
        let session = ctx.session();
        let batch = build_works_batch(&all_works)?;
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read Crossref batch: {e}")))?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// Arrow batch builder
// ---------------------------------------------------------------------------

fn build_works_batch(rows: &[Work]) -> Result<RecordBatch, DagError> {
    let dois: Vec<Option<String>> = rows.iter().map(|w| Some(w.doi.clone())).collect();
    let titles: Vec<Option<String>> = rows
        .iter()
        .map(|w| Some(w.title_str().to_string()))
        .collect();
    let types: Vec<Option<String>> = rows.iter().map(|w| Some(w.r#type.clone())).collect();
    let container_titles: Vec<Option<String>> = rows
        .iter()
        .map(|w| w.container_title.first().cloned())
        .collect();
    let publishers: Vec<Option<String>> = rows.iter().map(|w| Some(w.publisher.clone())).collect();
    let years: Vec<Option<i64>> = rows.iter().map(|w| w.year().map(|y| y as i64)).collect();
    let months: Vec<Option<i64>> = rows
        .iter()
        .map(|w| w.issued.as_ref().and_then(|d| d.ymd().1).map(|m| m as i64))
        .collect();
    let volumes: Vec<Option<String>> = rows.iter().map(|w| Some(w.volume.clone())).collect();
    let issues: Vec<Option<String>> = rows.iter().map(|w| Some(w.issue.clone())).collect();
    let pages: Vec<Option<String>> = rows.iter().map(|w| Some(w.page.clone())).collect();
    let issns: Vec<Option<String>> = rows.iter().map(|w| w.issn.first().cloned()).collect();
    let authors: Vec<Option<String>> = rows
        .iter()
        .map(|w| {
            if w.author.is_empty() {
                None
            } else {
                Some(
                    w.author
                        .iter()
                        .map(|a| a.display())
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            }
        })
        .collect();
    let cited_by: Vec<Option<u64>> = rows
        .iter()
        .map(|w| Some(w.is_referenced_by_count))
        .collect();
    let ref_counts: Vec<Option<u64>> = rows.iter().map(|w| Some(w.references_count)).collect();
    let abstracts: Vec<Option<String>> = rows.iter().map(|w| w.abstract_text.clone()).collect();
    let scores: Vec<Option<f64>> = rows.iter().map(|w| w.score).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("doi", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("type", DataType::Utf8, true),
        Field::new("container_title", DataType::Utf8, true),
        Field::new("publisher", DataType::Utf8, true),
        Field::new("year", DataType::Int64, true),
        Field::new("month", DataType::Int64, true),
        Field::new("volume", DataType::Utf8, true),
        Field::new("issue", DataType::Utf8, true),
        Field::new("page", DataType::Utf8, true),
        Field::new("issn", DataType::Utf8, true),
        Field::new("authors", DataType::Utf8, true),
        Field::new("cited_by_count", DataType::UInt64, true),
        Field::new("references_count", DataType::UInt64, true),
        Field::new("abstract", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            str_array(dois),
            str_array(titles),
            str_array(types),
            str_array(container_titles),
            str_array(publishers),
            Arc::new(Int64Array::from(years)),
            Arc::new(Int64Array::from(months)),
            str_array(volumes),
            str_array(issues),
            str_array(pages),
            str_array(issns),
            str_array(authors),
            Arc::new(UInt64Array::from(cited_by)),
            Arc::new(UInt64Array::from(ref_counts)),
            str_array(abstracts),
            Arc::new(Float64Array::from(scores)),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build Crossref works batch: {e}")))
}

// ---------------------------------------------------------------------------
// Arrow helpers
// ---------------------------------------------------------------------------

fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DateParts, Work};

    #[test]
    fn build_batch_from_works() {
        let works = vec![
            Work {
                doi: "10.1/a".into(),
                title: vec!["Paper A".into()],
                r#type: "journal-article".into(),
                container_title: vec!["Nature".into()],
                publisher: "Springer".into(),
                issued: Some(DateParts {
                    date_parts: vec![vec![Some(2024), Some(3)]],
                    ..Default::default()
                }),
                volume: "10".into(),
                issue: "1".into(),
                page: "1-10".into(),
                issn: vec!["1234-5678".into()],
                author: vec![crate::types::Author {
                    given: "Jane".into(),
                    family: "Smith".into(),
                    ..Default::default()
                }],
                is_referenced_by_count: 42,
                references_count: 15,
                score: Some(0.95),
                ..Default::default()
            },
            Work {
                doi: "10.2/b".into(),
                title: vec!["Paper B".into()],
                r#type: "book-chapter".into(),
                ..Default::default()
            },
        ];

        let batch = build_works_batch(&works).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 16);

        // DOI column.
        let dois = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(dois.value(0), "10.1/a");
        assert_eq!(dois.value(1), "10.2/b");

        // Year column.
        let years = batch
            .column(5)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(years.value(0), 2024);
        assert!(years.is_null(1));

        // Cited-by-count column.
        let cited = batch
            .column(12)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(cited.value(0), 42);
    }
}
