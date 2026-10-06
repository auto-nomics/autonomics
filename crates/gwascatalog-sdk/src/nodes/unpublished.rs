//! `source_gwascatalog_unpublished_studies` — unpublished GWAS Catalog
//! submissions as a table.

use std::sync::Arc;

use arrow_array::{BooleanArray, RecordBatch, UInt64Array};
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::rest::{EmbeddedUnpublishedStudies, RestPage, UnpublishedFilter, UnpublishedStudy};

/// Spec for [`UnpublishedNode`]. At least one filter is required — the
/// filter endpoint returns everything otherwise and the listing is huge.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct UnpublishedSpec {
    /// First author name.
    #[serde(default)]
    pub first_author: Option<String>,
    /// Study accession ID.
    #[serde(default)]
    pub accession: Option<String>,
    /// Title (partial match).
    #[serde(default)]
    pub title: Option<String>,
    /// Trait name.
    #[serde(default)]
    pub trait_name: Option<String>,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn unpublished_filter(spec: &UnpublishedSpec) -> Result<UnpublishedFilter, DagError> {
    let filter = UnpublishedFilter {
        first_author: spec.first_author.clone(),
        accession: spec.accession.clone(),
        title: spec.title.clone(),
        trait_: spec.trait_name.clone(),
    };
    let empty = filter.first_author.is_none()
        && filter.accession.is_none()
        && filter.title.is_none()
        && filter.trait_.is_none();
    if empty {
        return Err(DagError::Schedule(
            "source_gwascatalog_unpublished_studies requires at least one filter — \
             `first_author`, `accession`, `title`, or `trait_name`"
                .into(),
        ));
    }
    Ok(filter)
}

/// Source node emitting unpublished submissions.
#[derive(Clone)]
pub struct UnpublishedNode {
    meta: NodePorts,
    spec: UnpublishedSpec,
}

pub struct UnpublishedNodeFactory;

impl NodeFactory for UnpublishedNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_unpublished_studies"
    }

    fn desc(&self) -> &'static str {
        "Search GWAS Catalog unpublished study submissions and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over `GET /unpublished-studies/search/filter`. Requires \
         at least one filter: `first_author`, `accession`, `title`, or \
         `trait_name` (all given filters are AND-ed).\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates (default: one page).\n\n\
         Output schema: `study_accession, study_tag, study_description, \
         trait_name, efo_trait, genotyping_technology, imputation, \
         sample_description, cohort, variant_count`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UnpublishedSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: UnpublishedSpec = serde_json::from_value(spec)?;
        Ok(Box::new(UnpublishedNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for UnpublishedNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_unpublished_studies"
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
        let filter = unpublished_filter(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();

        let mut rows: Vec<UnpublishedStudy> = Vec::new();
        let mut page: Option<u32> = None;
        loop {
            let resp: RestPage<EmbeddedUnpublishedStudies> = client
                .rest_unpublished_studies(&filter, page, Some(page_size))
                .await
                .map_err(|e| {
                    DagError::Schedule(format!(
                        "GWAS Catalog unpublished-studies request failed: {e}"
                    ))
                })?;
            let batch_rows = resp
                ._embedded
                .map(|embedded| embedded.unpublished_studies)
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
            crate::nodes::utf8("study_accession"),
            crate::nodes::utf8("study_tag"),
            crate::nodes::utf8("study_description"),
            crate::nodes::utf8("trait_name"),
            crate::nodes::utf8("efo_trait"),
            crate::nodes::utf8("genotyping_technology"),
            arrow_schema::Field::new("imputation", arrow_schema::DataType::Boolean, true),
            crate::nodes::utf8("sample_description"),
            crate::nodes::utf8("cohort"),
            arrow_schema::Field::new("variant_count", arrow_schema::DataType::UInt64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                crate::nodes::str_array(
                    rows.iter()
                        .map(|s| Some(s.study_accession.clone()))
                        .collect(),
                ),
                crate::nodes::str_array(rows.iter().map(|s| Some(s.study_tag.clone())).collect()),
                crate::nodes::str_array(
                    rows.iter()
                        .map(|s| Some(s.study_description.clone()))
                        .collect(),
                ),
                crate::nodes::str_array(rows.iter().map(|s| Some(s.trait_name.clone())).collect()),
                crate::nodes::str_array(rows.iter().map(|s| s.efo_trait.clone()).collect()),
                crate::nodes::str_array(
                    rows.iter()
                        .map(|s| s.genotyping_technology.clone())
                        .collect(),
                ),
                Arc::new(BooleanArray::from(
                    rows.iter().map(|s| Some(s.imputation)).collect::<Vec<_>>(),
                )),
                crate::nodes::str_array(
                    rows.iter().map(|s| s.sample_description.clone()).collect(),
                ),
                crate::nodes::str_array(rows.iter().map(|s| s.cohort.clone()).collect()),
                Arc::new(UInt64Array::from(
                    rows.iter()
                        .map(|s| Some(s.variant_count))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| {
            DagError::Schedule(format!("failed to build unpublished-studies batch: {e}"))
        })?;
        let df = ctx.session().read_batch(batch).map_err(|e| {
            DagError::Schedule(format!("failed to read unpublished-studies batch: {e}"))
        })?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_filter_is_required() {
        let empty = UnpublishedSpec {
            first_author: None,
            accession: None,
            title: None,
            trait_name: None,
            page_size: None,
            max_results: None,
        };
        let err = unpublished_filter(&empty).unwrap_err().to_string();
        assert!(err.contains("at least one filter"), "{err}");
        assert!(
            unpublished_filter(&UnpublishedSpec {
                trait_name: Some("asthma".into()),
                ..empty
            })
            .is_ok()
        );
    }
}
