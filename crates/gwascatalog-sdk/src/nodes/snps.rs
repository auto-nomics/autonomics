//! `source_gwascatalog_snps` — GWAS Catalog SNP annotations as a table.

use std::sync::Arc;

use arrow_array::{RecordBatch, UInt32Array, UInt64Array};
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::rest::{EmbeddedSnps, RestPage, Snp, SnpQuery};

/// Spec for [`SnpsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct SnpsSpec {
    /// Variant rsID, e.g. `rs7329174`.
    #[serde(default)]
    pub rs_id: Option<String>,
    /// Gene name to find associated SNPs for.
    #[serde(default)]
    pub gene: Option<String>,
    /// Genomic range: `chrom:start-end`, e.g. `1:1000000-2000000`.
    #[serde(default)]
    pub genomic_range: Option<String>,
    /// Reported disease trait to find associated SNPs for.
    #[serde(default)]
    pub disease_trait: Option<String>,
    /// EFO trait label.
    #[serde(default)]
    pub efo_trait: Option<String>,
    /// PubMed ID.
    #[serde(default)]
    pub pubmed_id: Option<String>,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn snp_query(spec: &SnpsSpec) -> Result<SnpQuery, DagError> {
    if let Some(range) = spec.genomic_range.as_deref() {
        // The range form is exclusive of the other criteria in the API; a
        // range + anything else is rejected here rather than silently
        // dropping the extra criterion.
        let others = [
            spec.rs_id.is_some(),
            spec.gene.is_some(),
            spec.disease_trait.is_some(),
            spec.efo_trait.is_some(),
            spec.pubmed_id.is_some(),
        ];
        if others.iter().any(|&b| b) {
            return Err(DagError::Schedule(
                "source_gwascatalog_snps: `genomic_range` cannot be combined with the \
                 other search criteria"
                    .into(),
            ));
        }
        let bad = || {
            DagError::Schedule(format!(
                "source_gwascatalog_snps: `genomic_range` must look like \
                 `chrom:start-end`, e.g. `1:1000000-2000000`; got {range:?}"
            ))
        };
        let (chrom, span) = range.split_once(':').ok_or_else(bad)?;
        let (start, end) = span.split_once('-').ok_or_else(bad)?;
        let bp_start: u64 = start.parse().map_err(|_| bad())?;
        let bp_end: u64 = end.parse().map_err(|_| bad())?;
        return Ok(SnpQuery::ChromBpRange {
            chrom: chrom.to_string(),
            bp_start,
            bp_end,
        });
    }

    let named: Option<SnpQuery> = match (
        spec.rs_id.as_deref(),
        spec.gene.as_deref(),
        spec.disease_trait.as_deref(),
        spec.efo_trait.as_deref(),
        spec.pubmed_id.as_deref(),
    ) {
        (Some(rs), None, None, None, None) => Some(SnpQuery::RsId(rs.to_string())),
        (None, Some(g), None, None, None) => Some(SnpQuery::Gene(g.to_string())),
        (None, None, Some(t), None, None) => Some(SnpQuery::DiseaseTrait(t.to_string())),
        (None, None, None, Some(t), None) => Some(SnpQuery::EfoTrait(t.to_string())),
        (None, None, None, None, Some(pm)) => Some(SnpQuery::Pmid(pm.to_string())),
        _ => None,
    };
    named.ok_or_else(|| {
        DagError::Schedule(
            "source_gwascatalog_snps requires exactly one search criterion — `rs_id`, \
             `gene`, `genomic_range`, `disease_trait`, `efo_trait`, or `pubmed_id`"
                .into(),
        )
    })
}

/// Source node emitting SNP annotations.
#[derive(Clone)]
pub struct SnpsNode {
    meta: NodePorts,
    spec: SnpsSpec,
}

pub struct SnpsNodeFactory;

impl NodeFactory for SnpsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_snps"
    }

    fn desc(&self) -> &'static str {
        "Find GWAS Catalog SNP annotations (locations, genes) and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog SNP findBy* endpoints. Requires \
         exactly one search criterion: `rs_id`, `gene`, `genomic_range` \
         (`chrom:start-end`), `disease_trait`, `efo_trait`, or `pubmed_id`.\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates (default: one page).\n\n\
         Output schema: `rs_id, merged, functional_class, last_update_date, \
         chromosome, position, region, nearby_genes`.\n\n\
         For curated associations of a SNP use \
         `source_gwascatalog_associations` with `rs_id`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SnpsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SnpsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SnpsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for SnpsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_snps"
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
        let query = snp_query(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();

        let mut rows: Vec<Snp> = Vec::new();
        let mut page: Option<u32> = None;
        loop {
            let resp: RestPage<EmbeddedSnps> = client
                .rest_find_snps(&query, page, Some(page_size))
                .await
                .map_err(|e| DagError::Schedule(format!("GWAS Catalog SNP request failed: {e}")))?;
            let batch_rows = resp
                ._embedded
                .map(|embedded| embedded.single_nucleotide_polymorphisms)
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

        let batch = build_snps_batch(&rows)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read SNPs batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn build_snps_batch(rows: &[Snp]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        crate::nodes::utf8("rs_id"),
        arrow_schema::Field::new("merged", arrow_schema::DataType::UInt32, true),
        crate::nodes::utf8("functional_class"),
        crate::nodes::utf8("last_update_date"),
        crate::nodes::utf8("chromosome"),
        arrow_schema::Field::new("position", arrow_schema::DataType::UInt64, true),
        crate::nodes::utf8("region"),
        crate::nodes::utf8("nearby_genes"),
    ]));

    let chromosomes: Vec<Option<String>> = rows
        .iter()
        .map(|s| s.primary_location().map(|l| l.chromosome_name.clone()))
        .collect();
    let positions: Vec<Option<u64>> = rows
        .iter()
        .map(|s| s.primary_location().map(|l| l.chromosome_position))
        .collect();
    let regions: Vec<Option<String>> = rows
        .iter()
        .map(|s| {
            s.primary_location()
                .and_then(|l| l.region.as_ref()?.name.clone())
        })
        .collect();
    let nearby: Vec<Option<String>> = rows
        .iter()
        .map(|s| {
            crate::nodes::joined(
                &s.nearby_genes()
                    .iter()
                    .map(|g| g.to_string())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::str_array(rows.iter().map(|s| Some(s.rs_id.clone())).collect()),
            Arc::new(UInt32Array::from(
                rows.iter().map(|s| Some(s.merged)).collect::<Vec<_>>(),
            )),
            crate::nodes::str_array(rows.iter().map(|s| s.functional_class.clone()).collect()),
            crate::nodes::str_array(rows.iter().map(|s| s.last_update_date.clone()).collect()),
            crate::nodes::str_array(chromosomes),
            Arc::new(UInt64Array::from(positions)),
            crate::nodes::str_array(regions),
            crate::nodes::str_array(nearby),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build SNPs batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snp() -> Snp {
        serde_json::from_value(serde_json::json!({
            "rsId": "rs7329174",
            "merged": 0,
            "functionalClass": "missense_variant",
            "locations": [{
                "chromosomeName": "16",
                "chromosomePosition": 53_786_643,
                "region": { "name": "16q12.2" }
            }],
            "genomicContexts": [
                { "gene": { "geneName": "FTO" } },
                { "gene": { "geneName": "RPGRIPI1L" } }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn batch_extracts_location_and_nearby_genes() {
        let batch = build_snps_batch(&[snp()]).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 8);
        let chrom = batch
            .column(4)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(chrom.value(0), "16");
        let pos = batch
            .column(5)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(pos.value(0), 53_786_643);
        let genes = batch
            .column(7)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(genes.value(0), "FTO; RPGRIPI1L");
    }

    #[test]
    fn query_requires_exactly_one_criterion() {
        let empty = SnpsSpec {
            rs_id: None,
            gene: None,
            genomic_range: None,
            disease_trait: None,
            efo_trait: None,
            pubmed_id: None,
            page_size: None,
            max_results: None,
        };
        assert!(snp_query(&empty).is_err());

        let both = SnpsSpec {
            gene: Some("FTO".into()),
            disease_trait: Some("body mass index".into()),
            ..empty.clone()
        };
        assert!(snp_query(&both).is_err());

        let malformed = SnpsSpec {
            genomic_range: Some("1:1000".into()),
            ..empty
        };
        let err = snp_query(&malformed).unwrap_err().to_string();
        assert!(err.contains("chrom:start-end"), "{err}");
    }
}
