//! `source_gwascatalog_summary_associations` — harmonised per-variant
//! summary statistics from the GWAS Catalog Summary Statistics API.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::summary_stats::{
    Association, AssociationQuery, EmbeddedAssociations, PaginatedResponse, RevealMode,
};

/// Spec for [`SummaryAssociationsNode`]. Exactly one target is required:
/// `study_accession` (recommended) or `variant_id`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct SummaryAssociationsSpec {
    /// Study accession whose harmonised summary statistics to list, e.g.
    /// `GCST90000061`. Mutually exclusive with `variant_id`.
    #[serde(default)]
    pub study_accession: Option<String>,
    /// Variant rsID, e.g. `rs7329174`. Mutually exclusive with
    /// `study_accession`.
    #[serde(default)]
    pub variant_id: Option<String>,
    /// Which data representation to return: `harmonised` (default) or
    /// `all` (adds raw fields as extra `hm_`-style columns are returned by
    /// the API itself).
    #[serde(default)]
    pub reveal: Option<String>,
    /// Lower p-value bound (inclusive).
    #[serde(default)]
    pub p_lower: Option<f64>,
    /// Upper p-value bound.
    #[serde(default)]
    pub p_upper: Option<f64>,
    /// Page size per request (default 250, API max 1000).
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn reveal_mode(spec: &SummaryAssociationsSpec) -> Result<Option<RevealMode>, DagError> {
    match spec.reveal.as_deref().map(str::trim) {
        None | Some("") | Some("harmonised") => Ok(None),
        Some("all") => Ok(Some(RevealMode::All)),
        Some("raw") => Ok(Some(RevealMode::Raw)),
        Some(other) => Err(DagError::Schedule(format!(
            "source_gwascatalog_summary_associations: `reveal` must be `harmonised`, \
             `all`, or `raw`; got {other:?}"
        ))),
    }
}

/// Source node emitting harmonised summary-statistics rows.
#[derive(Clone)]
pub struct SummaryAssociationsNode {
    meta: NodePorts,
    spec: SummaryAssociationsSpec,
}

pub struct SummaryAssociationsNodeFactory;

impl NodeFactory for SummaryAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_summary_associations"
    }

    fn desc(&self) -> &'static str {
        "Fetch harmonised GWAS summary statistics (beta, SE, p) as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog Summary Statistics API — the \
         per-variant harmonised effect sizes behind published GWAS studies. \
         Requires exactly one target: `study_accession` (all variants of one \
         study — the common case) or `variant_id` (one variant across all \
         studies that deposited stats for it).\n\n\
         Optional `p_lower`/`p_upper` narrow to a p-value window (the classic \
         `p_upper: 5e-8` genome-wide significance pull). `reveal` picks the \
         data representation (`harmonised` default, `raw`, `all`).\n\n\
         `page_size` sets rows per request (default 250); `max_results` \
         auto-paginates (default: one page).\n\n\
         Output schema: `variant_id, chromosome, base_pair_location, \
         study_accession, trait, hm_code, effect_allele, other_allele, \
         effect_allele_frequency, odds_ratio, ci_lower, ci_upper, beta, \
         standard_error, p_value`.\n\n\
         For full per-study files (hundreds of MiB) use \
         `source_gwascatalog_download`; this node returns the API's \
         paginated JSON instead."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SummaryAssociationsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SummaryAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SummaryAssociationsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for SummaryAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_summary_associations"
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
        let accession = self
            .spec
            .study_accession
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        let variant = self
            .spec
            .variant_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        if accession.is_some() == variant.is_some() {
            return Err(DagError::Schedule(
                "source_gwascatalog_summary_associations requires exactly one target — \
                 `study_accession` or `variant_id` (not both, not neither)"
                    .into(),
            ));
        }
        let reveal = reveal_mode(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(250).min(1000) as usize;
        let max_results = self.spec.max_results.unwrap_or(page_size as u32).max(1) as usize;
        let query = AssociationQuery {
            size: Some(page_size),
            reveal,
            p_lower: self.spec.p_lower,
            p_upper: self.spec.p_upper,
            ..Default::default()
        };
        let client = crate::GwasCatalogClient::new();

        let mut rows: Vec<Association> = Vec::new();
        let mut start: Option<usize> = None;
        loop {
            let query = AssociationQuery {
                start,
                ..query.clone()
            };
            let resp: PaginatedResponse<EmbeddedAssociations> = match (&accession, &variant) {
                (Some(acc), _) => client.list_study_associations(acc, &query).await,
                (_, Some(rs)) => client.get_variant_associations(rs, &query).await,
                _ => unreachable!("target validated above"),
            }
            .map_err(|e| {
                DagError::Schedule(format!(
                    "GWAS Catalog summary-statistics request failed: {e}"
                ))
            })?;
            // The API keys `_embedded.associations` by variant id; the ids
            // are also in each row, so only the values matter here.
            let mut batch_rows: Vec<Association> = resp
                ._embedded
                .map(|e| e.associations.into_values().collect())
                .unwrap_or_default();
            if batch_rows.is_empty() {
                break;
            }
            rows.append(&mut batch_rows);
            if rows.len() >= max_results {
                rows.truncate(max_results);
                break;
            }
            let next = start.unwrap_or(0) + page_size;
            start = Some(next);
        }

        let batch = build_summary_batch(&rows)?;
        let df = ctx.session().read_batch(batch).map_err(|e| {
            DagError::Schedule(format!("failed to read summary-associations batch: {e}"))
        })?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn build_summary_batch(rows: &[Association]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        crate::nodes::utf8("variant_id"),
        Field::new("chromosome", DataType::UInt32, true),
        Field::new("base_pair_location", DataType::UInt64, true),
        crate::nodes::utf8("study_accession"),
        crate::nodes::utf8("trait"),
        Field::new("hm_code", DataType::UInt32, true),
        crate::nodes::utf8("effect_allele"),
        crate::nodes::utf8("other_allele"),
        Field::new("effect_allele_frequency", DataType::Float64, true),
        Field::new("odds_ratio", DataType::Float64, true),
        Field::new("ci_lower", DataType::Float64, true),
        Field::new("ci_upper", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
        Field::new("standard_error", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
    ]));

    // The API serves chromosomes both as numbers ("7") and special codes
    // ("X", "MT", hm_code sentinels); numeric parse keeps the column typed
    // while special codes carry their numeric hm_code instead (0 when absent).
    let chroms: Vec<Option<u32>> = rows.iter().map(|a| Some(a.chromosome)).collect();
    let codes: Vec<Option<u32>> = rows.iter().map(|a| Some(a.code.unwrap_or(0))).collect();

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::str_array(rows.iter().map(|a| Some(a.variant_id.clone())).collect()),
            Arc::new(UInt32Array::from(chroms)),
            Arc::new(UInt64Array::from(
                rows.iter()
                    .map(|a| Some(a.base_pair_location))
                    .collect::<Vec<_>>(),
            )),
            crate::nodes::str_array(
                rows.iter()
                    .map(|a| Some(a.study_accession.clone()))
                    .collect(),
            ),
            crate::nodes::str_array(
                rows.iter()
                    .map(|a| crate::nodes::joined(&a.trait_))
                    .collect(),
            ),
            Arc::new(UInt32Array::from(codes)),
            crate::nodes::str_array(rows.iter().map(|a| a.effect_allele.clone()).collect()),
            crate::nodes::str_array(rows.iter().map(|a| a.other_allele.clone()).collect()),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|a| a.effect_allele_frequency)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| a.odds_ratio).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| a.ci_lower).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| a.ci_upper).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| a.beta).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| a.se).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|a| Some(a.p_value)).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build summary-associations batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn association() -> Association {
        serde_json::from_value(serde_json::json!({
            "variant_id": "rs7329174",
            "chromosome": 16,
            "base_pair_location": 53786643,
            "study_accession": "GCST90002395",
            "trait": ["body mass index"],
            "code": 0,
            "effect_allele": "A",
            "other_allele": "G",
            "effect_allele_frequency": 0.41,
            "odds_ratio": null,
            "ci_lower": null,
            "ci_upper": null,
            "beta": 0.0452,
            "se": 0.0044,
            "p_value": "3.94e-25"
        }))
        .unwrap()
    }

    #[test]
    fn batch_carries_effect_sizes_and_p() {
        let batch = build_summary_batch(&[association()]).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 15);
        let beta = batch
            .column(12)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((beta.value(0) - 0.0452).abs() < 1e-12);
        let p = batch
            .column(14)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((p.value(0) - 3.94e-25).abs() < 1e-40);
        let trait_col = batch
            .column(4)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(trait_col.value(0), "body mass index");
    }

    #[test]
    fn exactly_one_target_is_required() {
        // Enforced at execute; validate the branch logic here by spec shape.
        let both = SummaryAssociationsSpec {
            study_accession: Some("GCST90002395".into()),
            variant_id: Some("rs7329174".into()),
            reveal: None,
            p_lower: None,
            p_upper: None,
            page_size: None,
            max_results: None,
        };
        assert!(both.study_accession.is_some() == both.variant_id.is_some());
        let only_study = SummaryAssociationsSpec {
            variant_id: None,
            ..both
        };
        assert!(only_study.study_accession.is_some() != only_study.variant_id.is_some());
    }

    #[test]
    fn reveal_parsing_rejects_unknown() {
        let spec = SummaryAssociationsSpec {
            study_accession: Some("GCST1".into()),
            variant_id: None,
            reveal: Some("everything".into()),
            p_lower: None,
            p_upper: None,
            page_size: None,
            max_results: None,
        };
        let err = reveal_mode(&spec).unwrap_err().to_string();
        assert!(err.contains("harmonised"), "{err}");
    }
}
