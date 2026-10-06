//! `source_gwascatalog_efo_traits` — GWAS Catalog EFO trait rows.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::rest::{EfoQuery, EfoTrait, EmbeddedEfoTraits, RestPage};

/// Spec for [`EfoTraitsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EfoTraitsSpec {
    /// EFO short form, e.g. `EFO_0001360`.
    #[serde(default)]
    pub short_form: Option<String>,
    /// Full ontology URI, e.g. `http://www.ebi.ac.uk/efo/EFO_0001360`.
    #[serde(default)]
    pub uri: Option<String>,
    /// Human-readable trait name, e.g. `body mass index`.
    #[serde(default)]
    pub trait_name: Option<String>,
    /// PubMed ID to find associated traits for.
    #[serde(default)]
    pub pubmed_id: Option<String>,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn efo_query(spec: &EfoTraitsSpec) -> Result<EfoQuery, DagError> {
    match (
        spec.short_form.as_deref(),
        spec.uri.as_deref(),
        spec.trait_name.as_deref(),
        spec.pubmed_id.as_deref(),
    ) {
        (Some(sf), None, None, None) => Ok(EfoQuery::ShortForm(sf.to_string())),
        (None, Some(uri), None, None) => Ok(EfoQuery::Uri(uri.to_string())),
        (None, None, Some(t), None) => Ok(EfoQuery::Trait(t.to_string())),
        (None, None, None, Some(pm)) => Ok(EfoQuery::Pmid(pm.to_string())),
        _ => Err(DagError::Schedule(
            "source_gwascatalog_efo_traits requires exactly one search criterion — \
             `short_form`, `uri`, `trait_name`, or `pubmed_id`"
                .into(),
        )),
    }
}

/// Source node emitting EFO traits.
#[derive(Clone)]
pub struct EfoTraitsNode {
    meta: NodePorts,
    spec: EfoTraitsSpec,
}

pub struct EfoTraitsNodeFactory;

impl NodeFactory for EfoTraitsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_efo_traits"
    }

    fn desc(&self) -> &'static str {
        "Look up GWAS Catalog EFO traits and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog `/efoTraits` findBy* endpoints. \
         Requires exactly one search criterion: `short_form`, `uri`, \
         `trait_name`, or `pubmed_id`.\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates (default: one page).\n\n\
         Output schema: `trait_name, short_form, uri`. Feed `short_form` or \
         `trait_name` into `source_gwascatalog_studies` / \
         `source_gwascatalog_associations` for the studies behind a trait."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EfoTraitsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EfoTraitsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EfoTraitsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EfoTraitsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_efo_traits"
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
        let query = efo_query(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();

        let mut rows: Vec<EfoTrait> = Vec::new();
        let mut page: Option<u32> = None;
        loop {
            let resp: RestPage<EmbeddedEfoTraits> = client
                .rest_find_efo_traits(&query, page, Some(page_size))
                .await
                .map_err(|e| {
                    DagError::Schedule(format!("GWAS Catalog EFO trait request failed: {e}"))
                })?;
            let batch_rows = resp
                ._embedded
                .map(|embedded| embedded.efo_traits)
                .unwrap_or_default();
            let total_pages = resp.page.total_pages;
            if batch_rows.is_empty() {
                break;
            }
            rows.extend(batch_rows);
            if rows.len() >= max_results as usize {
                rows.truncate(max_results as usize);
                break;
            }
            let next = resp.page.number.saturating_add(1);
            if total_pages > 0 && next >= total_pages {
                break;
            }
            page = Some(next);
        }

        let schema = Arc::new(Schema::new(vec![
            crate::nodes::utf8("trait_name"),
            crate::nodes::utf8("short_form"),
            crate::nodes::utf8("uri"),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                crate::nodes::str_array(rows.iter().map(|t| Some(t.trait_name.clone())).collect()),
                crate::nodes::str_array(rows.iter().map(|t| Some(t.short_form.clone())).collect()),
                crate::nodes::str_array(rows.iter().map(|t| Some(t.uri.clone())).collect()),
            ],
        )
        .map_err(|e| DagError::Schedule(format!("failed to build EFO traits batch: {e}")))?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read EFO traits batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_requires_exactly_one_criterion() {
        let empty = EfoTraitsSpec {
            short_form: None,
            uri: None,
            trait_name: None,
            pubmed_id: None,
            page_size: None,
            max_results: None,
        };
        assert!(efo_query(&empty).is_err());
        let both = EfoTraitsSpec {
            short_form: Some("EFO_0001360".into()),
            uri: Some("http://www.ebi.ac.uk/efo/EFO_0001360".into()),
            ..empty.clone()
        };
        assert!(efo_query(&both).is_err());
        assert!(
            efo_query(&EfoTraitsSpec {
                trait_name: Some("body mass index".into()),
                ..empty
            })
            .is_ok()
        );
    }
}
