//! `source_gwascatalog_search` — Solr full-text search across GWAS Catalog
//! resources as a normalized table.

use std::sync::Arc;

use arrow_array::{BooleanArray, RecordBatch, UInt64Array};
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::search::{SearchDoc, SearchFilter};

/// Spec for [`SearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct SearchSpec {
    /// Solr query expression. Required — `*:*` matches all; field-specific
    /// syntax like `resourcename:study AND "breast cancer"` narrows it.
    pub q: String,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
    /// P-value range filter, e.g. `1e-8-1e-5`.
    #[serde(default)]
    pub pval_filter: Option<String>,
    /// Odds-ratio range filter, e.g. `1.5-3.0`.
    #[serde(default)]
    pub or_filter: Option<String>,
    /// Beta-coefficient range filter, e.g. `0-1`.
    #[serde(default)]
    pub beta_filter: Option<String>,
    /// Publication-date range filter, e.g. `2020-2024`.
    #[serde(default)]
    pub date_filter: Option<String>,
    /// Genomic location filter: `chrom:start-end`, e.g. `1:1000000-2000000`.
    #[serde(default)]
    pub genomic_filter: Option<String>,
    /// EFO trait ID(s) to filter by, e.g. `["EFO_0000400"]`.
    #[serde(default)]
    pub trait_filter: Vec<String>,
    /// Genotyping technology filter(s).
    #[serde(default)]
    pub genotyping_tech_filter: Vec<String>,
    /// Ancestry label filter(s).
    #[serde(default)]
    pub ancestry_filter: Vec<String>,
}

fn search_filter(spec: &SearchSpec, page_size: u32) -> SearchFilter {
    SearchFilter {
        q: spec.q.clone(),
        max: Some(page_size),
        start: None,
        pval_filter: spec.pval_filter.clone(),
        or_filter: spec.or_filter.clone(),
        beta_filter: spec.beta_filter.clone(),
        date_filter: spec.date_filter.clone(),
        genomic_filter: spec.genomic_filter.clone(),
        trait_filter: spec.trait_filter.clone(),
        genotyping_tech_filter: spec.genotyping_tech_filter.clone(),
        ancestry_filter: spec.ancestry_filter.clone(),
    }
}

/// Source node emitting Solr search docs.
#[derive(Clone)]
pub struct SearchNode {
    meta: NodePorts,
    spec: SearchSpec,
}

pub struct SearchNodeFactory;

impl NodeFactory for SearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_search"
    }

    fn desc(&self) -> &'static str {
        "Full-text search across all GWAS Catalog resources; one normalized row per hit."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog Solr Search API. `q` is required \
         (`*:*` matches everything; `resourcename:study AND \"breast cancer\"` \
         narrows by type and text). Numeric/range/array filters (`pval_filter`, \
         `or_filter`, `beta_filter`, `date_filter`, `genomic_filter`, \
         `trait_filter`, `genotyping_tech_filter`, `ancestry_filter`) map \
         straight onto the API's filter parameters.\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates via `start` (default: one page).\n\n\
         Docs are heterogeneous (fields vary by resource type) and are \
         normalized into one row shape: `resourcename, label, accession_id, \
         title, rs_id, chromosome_name, chromosome_position, region, \
         consequence, mapped_genes, mapped_trait, short_form, ensembl_id, \
         entrez_id, pmid, journal, publication_date, association_count, \
         study_count, full_pvalue_set`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SearchSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SearchNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for SearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_search"
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
        let q = self.spec.q.trim().to_string();
        if q.is_empty() {
            return Err(DagError::Schedule(
                "source_gwascatalog_search requires a non-empty `q`".into(),
            ));
        }
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();

        let mut docs: Vec<SearchDoc> = Vec::new();
        let mut start: u32 = 0;
        loop {
            let mut filter = search_filter(&self.spec, page_size);
            filter.q = q.clone();
            filter.start = Some(start);
            let resp = client
                .search(&filter)
                .await
                .map_err(|e| DagError::Schedule(format!("GWAS Catalog search failed: {e}")))?;
            let num_found = resp.response.num_found;
            if resp.response.docs.is_empty() {
                break;
            }
            docs.extend(resp.response.docs);
            if docs.len() >= max_results as usize {
                docs.truncate(max_results as usize);
                break;
            }
            start += page_size;
            if start as u64 >= num_found {
                break;
            }
        }

        let batch = build_search_batch(&docs)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read search batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn build_search_batch(docs: &[SearchDoc]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        crate::nodes::utf8("resourcename"),
        crate::nodes::utf8("label"),
        crate::nodes::utf8("accession_id"),
        crate::nodes::utf8("title"),
        crate::nodes::utf8("rs_id"),
        crate::nodes::utf8("chromosome_name"),
        crate::nodes::utf8("chromosome_position"),
        crate::nodes::utf8("region"),
        crate::nodes::utf8("consequence"),
        crate::nodes::utf8("mapped_genes"),
        crate::nodes::utf8("mapped_trait"),
        crate::nodes::utf8("short_form"),
        crate::nodes::utf8("ensembl_id"),
        crate::nodes::utf8("entrez_id"),
        crate::nodes::utf8("pmid"),
        crate::nodes::utf8("journal"),
        crate::nodes::utf8("publication_date"),
        arrow_schema::Field::new("association_count", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("study_count", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("full_pvalue_set", arrow_schema::DataType::Boolean, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::str_array(docs.iter().map(|d| d.resourcename.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| Some(d.label())).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.accession_id.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.title.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.rs_id.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.chromosome_name.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.chromosome_position.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.region.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.consequence.clone()).collect()),
            crate::nodes::str_array(
                docs.iter()
                    .map(|d| crate::nodes::joined(&d.mapped_genes))
                    .collect(),
            ),
            crate::nodes::str_array(docs.iter().map(|d| d.mapped_trait.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.short_form.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.ensembl_id.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.entrez_id.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.pmid.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.journal.clone()).collect()),
            crate::nodes::str_array(docs.iter().map(|d| d.publication_date.clone()).collect()),
            Arc::new(UInt64Array::from(
                docs.iter().map(|d| d.association_count).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                docs.iter().map(|d| d.study_count).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                docs.iter().map(|d| d.full_pvalue_set).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build search batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;

    fn doc(resourcename: &str) -> SearchDoc {
        let mut d = SearchDoc::default();
        d.resourcename = Some(resourcename.into());
        match resourcename {
            "study" => {
                d.accession_id = Some("GCST005038".into());
                d.title = Some("GWAS of body mass index".into());
                d.association_count = Some(1_240);
                d.full_pvalue_set = Some(true);
            }
            "variant" => {
                d.rs_id = Some("rs7329174".into());
                d.chromosome_name = Some("16".into());
                d.mapped_genes = vec!["FTO".into(), "IRX3".into()];
            }
            _ => {
                d.mapped_trait = Some("body mass index".into());
                d.short_form = Some("EFO_0004343".into());
            }
        }
        d
    }

    #[test]
    fn heterogeneous_docs_normalize_to_one_row_shape() {
        let docs = vec![doc("study"), doc("variant"), doc("trait")];
        let batch = build_search_batch(&docs).unwrap();
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.num_columns(), 20);
        let resource = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(resource.value(0), "study");
        assert_eq!(resource.value(1), "variant");
        let genes = batch
            .column(9)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(genes.value(1), "FTO; IRX3");
        assert!(genes.is_null(0));
    }

    fn test_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }
}
