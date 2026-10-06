//! `source_gwascatalog_associations` and
//! `source_gwascatalog_study_associations` — curated SNP-trait association
//! rows from the GWAS Catalog REST API.

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int32Array, RecordBatch, UInt32Array};
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::rest::{AssociationQueryKind, EmbeddedRestAssociations, RestAssociation, RestPage};

/// Spec for [`AssociationsNode`] — one search criterion per request.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AssociationsSpec {
    /// Variant rsID, e.g. `rs7329174`.
    #[serde(default)]
    pub rs_id: Option<String>,
    /// Study accession ID, e.g. `GCST005038`. Combined with `rs_id` when
    /// both are given (the API has a findByRsIdAndAccessionId endpoint).
    #[serde(default)]
    pub accession_id: Option<String>,
    /// PubMed ID to find associations for.
    #[serde(default)]
    pub pubmed_id: Option<String>,
    /// EFO trait label.
    #[serde(default)]
    pub efo_trait: Option<String>,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

/// Spec for [`StudyAssociationsNode`] — all associations of one study.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct StudyAssociationsSpec {
    /// Study accession ID, e.g. `GCST005038`.
    pub accession_id: String,
    /// Page size per request (default 20, max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

/// Resolve the query kind for a find-associations spec.
fn association_query(spec: &AssociationsSpec) -> Result<AssociationQueryKind, DagError> {
    match (
        spec.rs_id.as_deref(),
        spec.accession_id.as_deref(),
        spec.pubmed_id.as_deref(),
        spec.efo_trait.as_deref(),
    ) {
        (Some(rs), Some(acc), None, None) => Ok(AssociationQueryKind::RsIdAndAccession {
            rs_id: rs.to_string(),
            accession: acc.to_string(),
        }),
        (Some(rs), None, None, None) => Ok(AssociationQueryKind::RsId(rs.to_string())),
        (None, Some(acc), None, None) => Ok(AssociationQueryKind::StudyAccession(acc.to_string())),
        (None, None, Some(pm), None) => Ok(AssociationQueryKind::Pmid(pm.to_string())),
        (None, None, None, Some(t)) => Ok(AssociationQueryKind::EfoTrait(t.to_string())),
        _ => Err(DagError::Schedule(
            "source_gwascatalog_associations requires exactly one search criterion — \
             `rs_id`, `accession_id`, `pubmed_id`, or `efo_trait` (rs_id and \
             accession_id may be combined); the REST API has one findBy* \
             endpoint per request"
                .into(),
        )),
    }
}

/// Fetch associations across pages until `max_results` or the last page.
async fn fetch_associations<F, Fut>(
    page_size: u32,
    max_results: u32,
    fetch_page: F,
) -> Result<Vec<RestAssociation>, DagError>
where
    F: Fn(Option<u32>, u32) -> Fut,
    Fut: std::future::Future<
            Output = Result<RestPage<EmbeddedRestAssociations>, crate::GwasCatalogError>,
        >,
{
    let mut out = Vec::new();
    let mut page: Option<u32> = None;
    loop {
        let resp = fetch_page(page, page_size).await.map_err(|e| {
            DagError::Schedule(format!("GWAS Catalog associations request failed: {e}"))
        })?;
        let rows = resp
            ._embedded
            .map(|embedded| embedded.associations)
            .unwrap_or_default();
        let total_pages = resp.page.total_pages;
        if rows.is_empty() {
            return Ok(out);
        }
        out.extend(rows);
        if out.len() >= max_results as usize {
            out.truncate(max_results as usize);
            return Ok(out);
        }
        let next = resp.page.number.saturating_add(1);
        if total_pages > 0 && next >= total_pages {
            return Ok(out);
        }
        page = Some(next);
    }
}

/// Source node emitting curated associations matching one criterion.
#[derive(Clone)]
pub struct AssociationsNode {
    meta: NodePorts,
    spec: AssociationsSpec,
}

pub struct AssociationsNodeFactory;

impl NodeFactory for AssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_associations"
    }

    fn desc(&self) -> &'static str {
        "Find GWAS Catalog curated SNP-trait associations and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog REST association findBy* \
         endpoints. Requires exactly one search criterion: `rs_id`, \
         `accession_id`, `pubmed_id`, or `efo_trait` (`rs_id` + \
         `accession_id` may be combined). For every association of one \
         study use `source_gwascatalog_study_associations`.\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates (default: one page).\n\n\
         Output schema: `rs_id, risk_allele, risk_frequency, pvalue, \
         pvalue_mantissa, pvalue_exponent, or_per_copy_num, beta_num, \
         beta_unit, beta_direction, standard_error, range, snp_type, \
         multi_snp_haplotype, snp_interaction, genes, entrez_ids`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AssociationsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: AssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AssociationsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for AssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_associations"
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
        let query = association_query(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();
        let rows = fetch_associations(page_size, max_results, |page, size| {
            client.rest_find_associations(&query, page, Some(size))
        })
        .await?;
        emit_association_rows(ctx, &rows)
    }
}

/// Source node emitting every curated association of one study.
#[derive(Clone)]
pub struct StudyAssociationsNode {
    meta: NodePorts,
    spec: StudyAssociationsSpec,
}

pub struct StudyAssociationsNodeFactory;

impl NodeFactory for StudyAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_study_associations"
    }

    fn desc(&self) -> &'static str {
        "Fetch all curated associations of one GWAS Catalog study."
    }

    fn doc(&self) -> &'static str {
        "A source node over `GET /studies/{accessionId}/associations`. \
         Emits every curated SNP-trait association row of the study with the \
         same schema as `source_gwascatalog_associations` plus a leading \
         `study_accession` column.\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates (default: one page)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StudyAssociationsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: StudyAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StudyAssociationsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for StudyAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_study_associations"
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
        let accession = self.spec.accession_id.trim().to_string();
        if accession.is_empty() {
            return Err(DagError::Schedule(
                "source_gwascatalog_study_associations requires `accession_id`".into(),
            ));
        }
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();
        let rows = fetch_associations(page_size, max_results, |page, size| {
            client.rest_study_associations(&accession, page, Some(size))
        })
        .await?;

        let batch = build_associations_batch(&rows, Some(&accession))?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read associations batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn emit_association_rows(ctx: &NodeCtx, rows: &[RestAssociation]) -> Result<PortOutputs, DagError> {
    let batch = build_associations_batch(rows, None)?;
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read associations batch: {e}")))?;
    let mut outputs = PortOutputs::new();
    outputs.insert(0, df);
    Ok(outputs)
}

/// Arrow batch builder shared by both association kinds. `study_accession`
/// is Some only for the per-study kind (leading column).
fn build_associations_batch(
    rows: &[RestAssociation],
    study_accession: Option<&str>,
) -> Result<arrow_array::RecordBatch, DagError> {
    let mut fields = Vec::new();
    if study_accession.is_some() {
        fields.push(crate::nodes::utf8("study_accession"));
    }
    fields.extend([
        crate::nodes::utf8("rs_id"),
        crate::nodes::utf8("risk_allele"),
        crate::nodes::utf8("risk_frequency"),
        arrow_schema::Field::new("pvalue", arrow_schema::DataType::Float64, true),
        arrow_schema::Field::new("pvalue_mantissa", arrow_schema::DataType::UInt32, true),
        arrow_schema::Field::new("pvalue_exponent", arrow_schema::DataType::Int32, true),
        arrow_schema::Field::new("or_per_copy_num", arrow_schema::DataType::Float64, true),
        arrow_schema::Field::new("beta_num", arrow_schema::DataType::Float64, true),
        crate::nodes::utf8("beta_unit"),
        crate::nodes::utf8("beta_direction"),
        arrow_schema::Field::new("standard_error", arrow_schema::DataType::Float64, true),
        crate::nodes::utf8("range"),
        crate::nodes::utf8("snp_type"),
        arrow_schema::Field::new("multi_snp_haplotype", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("snp_interaction", arrow_schema::DataType::Boolean, true),
        crate::nodes::utf8("genes"),
        crate::nodes::utf8("entrez_ids"),
    ]);
    let schema = Arc::new(Schema::new(fields));

    let mut columns: Vec<Arc<dyn arrow_array::Array>> = Vec::new();
    if let Some(accession) = study_accession {
        let col: Vec<Option<String>> = rows.iter().map(|_| Some(accession.to_string())).collect();
        columns.push(crate::nodes::str_array(col));
    }
    columns.extend([
        crate::nodes::str_array(rows.iter().map(|a| a.rsid().map(str::to_string)).collect()),
        crate::nodes::str_array(
            rows.iter()
                .map(|a| {
                    a.loci
                        .first()
                        .and_then(|l| l.strongest_risk_alleles.first())
                        .and_then(|allele| allele.risk_allele_name.clone())
                })
                .collect(),
        ),
        crate::nodes::str_array(rows.iter().map(|a| a.risk_frequency.clone()).collect()),
        Arc::new(Float64Array::from(
            rows.iter().map(|a| a.pvalue).collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|a| a.pvalue_mantissa).collect::<Vec<_>>(),
        )),
        Arc::new(Int32Array::from(
            rows.iter().map(|a| a.pvalue_exponent).collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            rows.iter().map(|a| a.or_per_copy_num).collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            rows.iter().map(|a| a.beta_num).collect::<Vec<_>>(),
        )),
        crate::nodes::str_array(rows.iter().map(|a| a.beta_unit.clone()).collect()),
        crate::nodes::str_array(rows.iter().map(|a| a.beta_direction.clone()).collect()),
        Arc::new(Float64Array::from(
            rows.iter().map(|a| a.standard_error).collect::<Vec<_>>(),
        )),
        crate::nodes::str_array(rows.iter().map(|a| a.range.clone()).collect()),
        crate::nodes::str_array(rows.iter().map(|a| a.snp_type.clone()).collect()),
        Arc::new(BooleanArray::from(
            rows.iter()
                .map(|a| Some(a.multi_snp_haplotype))
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter()
                .map(|a| Some(a.snp_interaction))
                .collect::<Vec<_>>(),
        )),
        crate::nodes::str_array(
            rows.iter()
                .map(|a| {
                    crate::nodes::joined(
                        &a.genes().iter().map(|g| g.to_string()).collect::<Vec<_>>(),
                    )
                })
                .collect(),
        ),
        crate::nodes::str_array(
            rows.iter()
                .map(|a| {
                    crate::nodes::joined(
                        &a.entrez_ids()
                            .iter()
                            .map(|g| g.to_string())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect(),
        ),
    ]);

    RecordBatch::try_new(schema, columns)
        .map_err(|e| DagError::Schedule(format!("failed to build associations batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn association() -> RestAssociation {
        serde_json::from_value(serde_json::json!({
            "riskFrequency": "0.42",
            "pvalueMantissa": 5,
            "pvalueExponent": -9,
            "pvalue": 5e-9,
            "orPerCopyNum": 1.18,
            "betaNum": 0.16,
            "betaUnit": "SD",
            "betaDirection": "increase",
            "standardError": 0.02,
            "snpType": "novel",
            "multiSnpHaplotype": false,
            "snpInteraction": false,
            "loci": [{
                "strongestRiskAlleles": [{
                    "riskAlleleName": "rs7329174-A",
                    "_links": { "snp": { "href":
                        "https://www.ebi.ac.uk/gwas/rest/api/singleNucleotidePolymorphisms/rs7329174" } }
                }],
                "authorReportedGenes": [{
                    "geneName": "FTO",
                    "entrezGeneIds": [{ "entrezGeneId": "79068" }]
                }]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn batch_extracts_rsid_allele_and_genes() {
        let rows = vec![association()];
        let batch = build_associations_batch(&rows, None).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 17);
        let rs = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(rs.value(0), "rs7329174");
        let allele = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(allele.value(0), "rs7329174-A");
        let genes = batch
            .column(15)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(genes.value(0), "FTO");
    }

    #[test]
    fn study_kind_prepends_accession_column() {
        let rows = vec![association()];
        let batch = build_associations_batch(&rows, Some("GCST005038")).unwrap();
        assert_eq!(batch.num_columns(), 18);
        let accession = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(accession.value(0), "GCST005038");
    }

    #[test]
    fn criterion_validation_rejects_empty_and_multi() {
        let empty = AssociationsSpec {
            rs_id: None,
            accession_id: None,
            pubmed_id: None,
            efo_trait: None,
            page_size: None,
            max_results: None,
        };
        assert!(association_query(&empty).is_err());

        let both = AssociationsSpec {
            pubmed_id: Some("123".into()),
            efo_trait: Some("bmi".into()),
            ..empty
        };
        let err = association_query(&both).unwrap_err().to_string();
        assert!(err.contains("exactly one search criterion"), "{err}");
    }
}
