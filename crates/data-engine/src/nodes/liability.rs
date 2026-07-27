//! Liability-scale heritability conversion node.
//!
//! A pure post-hoc transform that converts an **observed-scale** SNP-heritability
//! estimate (produced by [`super::ldsc_hsq::LdscHsqNode`]) to the **liability
//! scale** for an ascertained (case-control) binary phenotype, using the
//! standard liability-threshold conversion of So et al. (2011) / Lee et al.
//!
//! This mirrors how the original LDSC handles the conversion: the regression
//! itself (`reg.Hsq`) always emits observed-scale h², and the liability
//! conversion is a separate display-layer step applied only to `h²` and `h²_se`
//! via a constant multiplier (see `ldscore/regressions.py: Hsq.summary` and
//! `h2_obs_to_liab`). Keeping it in its own node — rather than folding `P`/`K`
//! into [`LdscHsqConfig`] — means:
//!
//! * the expensive LD score regression is not re-run when `P`/`K` change;
//! * the conversion is reusable for any observed-scale h² summary; and
//! * the observed-scale columns are preserved alongside the liability columns.
//!
//! Only `h2` and `h2_se` are converted. The intercept, ratio, λ_GC, and meanχ²
//! live on the χ² / observed scale and are **not** convertible, so they pass
//! through unchanged.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::NodeFactory,
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LiabilityNodeError {
    #[error("liability conversion failed: {0}")]
    Ldsc(#[from] ldsc::LdscError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read upstream batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("liability node requires exactly one input h² summary DataFrame")]
    NoInput,
    #[error("upstream h² summary has no rows")]
    EmptyInput,
    #[error("upstream batch is missing required Float64 column '{0}'")]
    MissingColumn(String),
}

impl From<LiabilityNodeError> for DagError {
    fn from(e: LiabilityNodeError) -> Self {
        DagError::NodeError {
            node_type: "liability".to_string(),
            msg: e.to_string(),
        }
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// Added liability-scale column names.
const H2_LIAB_COL: &str = "h2_liab";
const H2_SE_LIAB_COL: &str = "h2_se_liab";
/// Observed-scale columns read from the upstream h² summary.
const H2_COL: &str = "h2";
const H2_SE_COL: &str = "h2_se";

/// The output schema: the upstream h² summary schema (see
/// [`super::ldsc_hsq::output_schema`]) with two added liability-scale columns
/// `h2_liab` and `h2_se_liab` (Float64, non-nullable — `h2`/`h2_se` are
/// non-nullable upstream and the conversion factor is finite for valid `P`/`K`).
pub fn output_schema() -> SchemaRef {
    let mut fields: Vec<Arc<Field>> = super::ldsc_hsq::output_schema()
        .fields()
        .iter()
        .cloned()
        .collect();
    fields.push(Arc::new(Field::new(H2_LIAB_COL, DataType::Float64, false)));
    fields.push(Arc::new(Field::new(
        H2_SE_LIAB_COL,
        DataType::Float64,
        false,
    )));
    Arc::new(Schema::new(fields))
}

/// Static port layout: one typed input (the `ldsc` h² summary) and one typed
/// output (that summary + the two liability columns). Typing the input to
/// [`super::ldsc_hsq::output_schema`] lets the DAG reject non-h² upstreams at
/// `add_edge` time.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(super::ldsc_hsq::output_schema()))
        .add_output_port(Some(output_schema()))
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the liability-scale conversion.
///
/// Both prevalences are required — the conversion is only defined when the
/// sample and population ascertainment of a binary phenotype are known. For a
/// quantitative trait, do not use this node (observed scale == liability scale).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LiabilityConfig {
    /// Sample prevalence `P` — the proportion of cases in the study sample
    /// (`--samp-prev` in the original LDSC CLI). Must be in `(0, 1)`.
    pub samp_prev: f64,
    /// Population prevalence `K` — the proportion of cases in the underlying
    /// population (`--pop-prev`). Must be in `(0, 1)`.
    pub pop_prev: f64,
}

impl LiabilityConfig {
    /// Construct a [`LiabilityConfig`] from sample prevalence `P` and
    /// population prevalence `K`.
    pub fn new(samp_prev: f64, pop_prev: f64) -> Self {
        Self {
            samp_prev,
            pop_prev,
        }
    }
}

// =====================================================================
// Node + factory
// =====================================================================

/// Convert an observed-scale h² summary to the liability scale.
///
/// Accepts the single-row output of [`super::ldsc_hsq::LdscHsqNode`] and emits
/// it with two added columns, `h2_liab = c · h2` and `h2_se_liab = c · h2_se`,
/// where `c` is the liability-threshold conversion factor
/// `K²(1−K)² / [P(1−P)·φ(Φ⁻¹(1−K))²]`. All other columns pass through
/// unchanged.
#[derive(Clone)]
pub struct LiabilityNode {
    meta: NodePorts,
    cfg: LiabilityConfig,
}

pub struct LiabilityNodeFactory {}

impl NodeFactory for LiabilityNodeFactory {
    fn kind(&self) -> &'static str {
        "liability"
    }

    fn desc(&self) -> &'static str {
        "Convert observed-scale SNP-heritability (h²) to the liability scale for a binary phenotype."
    }

    fn doc(&self) -> &'static str {
        "Liability-scale heritability conversion node. Takes the single-row \
        h² summary produced by the `ldsc` node and appends two columns, \
        `h2_liab` and `h2_se_liab`, obtained by multiplying `h2` and `h2_se` \
        by the liability-threshold conversion factor \
        K²(1−K)² / [P(1−P)·φ(Φ⁻¹(1−K))²], where P is the sample prevalence \
        (`samp_prev`) and K is the population prevalence (`pop_prev`). Only \
        `h2`/`h2_se` are converted; the intercept, ratio, λ_GC, and meanχ² \
        are on the χ² scale and pass through unchanged. Use only for \
        binary/case-control phenotypes; for quantitative traits observed \
        scale already equals liability scale."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LiabilityConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: crate::node_registry::registry::NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let cfg: LiabilityConfig = serde_json::from_value(spec)?;
        Ok(Box::new(LiabilityNode::new(cfg)))
    }
}

impl LiabilityNode {
    /// Construct a [`LiabilityNode`] with the given prevalences.
    pub fn new(cfg: LiabilityConfig) -> Self {
        Self {
            meta: port_layout(),
            cfg,
        }
    }

    /// The liability-threshold conversion factor `c` such that
    /// `h²_liab = c · h²_obs`. Ported from `ldsc::regress::h2_obs_to_liab`
    /// (itself a port of `regressions.py: h2_obs_to_liab`), evaluated with
    /// `h²_obs = 1` so the result is the pure multiplier.
    fn conversion_factor(&self) -> Result<f64, LiabilityNodeError> {
        Ok(ldsc::regress::h2_obs_to_liab(
            1.0,
            self.cfg.samp_prev,
            self.cfg.pop_prev,
        )?)
    }

    /// Apply the conversion to a set of upstream `RecordBatch`es, appending
    /// `h2_liab` and `h2_se_liab` to each. Extracted from
    /// [`DagNode::execute`](LiabilityNode::execute) so the transform can be
    /// tested without a live DataFusion context.
    fn apply(&self, batches: &[RecordBatch]) -> Result<Vec<RecordBatch>, LiabilityNodeError> {
        if batches.is_empty() {
            return Err(LiabilityNodeError::EmptyInput);
        }
        let c = self.conversion_factor()?;
        let out_schema = output_schema();

        let mut out = Vec::with_capacity(batches.len());
        for b in batches {
            let h2 = col_f64(b, H2_COL)?;
            let hse = col_f64(b, H2_SE_COL)?;
            let n = b.num_rows();

            // h2/h2_se are non-nullable upstream, but guard anyway so a stray
            // null becomes a null rather than a panic.
            let mut h2_liab = Vec::with_capacity(n);
            let mut hse_liab = Vec::with_capacity(n);
            for i in 0..n {
                if h2.is_null(i) || hse.is_null(i) {
                    h2_liab.push(None);
                    hse_liab.push(None);
                } else {
                    h2_liab.push(Some(c * h2.value(i)));
                    hse_liab.push(Some(c * hse.value(i)));
                }
            }

            // Pass through every upstream column, then append the two new ones.
            let mut cols: Vec<Arc<dyn Array>> = b.columns().to_vec();
            cols.push(Arc::new(Float64Array::from_iter(h2_liab)));
            cols.push(Arc::new(Float64Array::from_iter(hse_liab)));
            out.push(RecordBatch::try_new(out_schema.clone(), cols)?);
        }
        Ok(out)
    }
}

/// Downcast a named column of `b` to `&Float64Array`.
fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, LiabilityNodeError> {
    let idx = b
        .schema()
        .index_of(name)
        .map_err(|_| LiabilityNodeError::MissingColumn(name.to_string()))?;
    b.column(idx)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| LiabilityNodeError::MissingColumn(name.to_string()))
}

#[async_trait]
impl DagNode for LiabilityNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "liability"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(LiabilityNodeError::NoInput)?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(LiabilityNodeError::ReadBatch)?;
        if batches.is_empty() {
            return Err(LiabilityNodeError::EmptyInput.into());
        }

        let out_batches = self.apply(&batches)?;

        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx
            .read_batches(out_batches)
            .map_err(LiabilityNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float64Array, StringArray};

    /// Build a single-row h² summary `RecordBatch` matching
    /// [`crate::nodes::ldsc_hsq::output_schema`] with the given h² / h²_se.
    fn h2_summary_batch(h2: f64, h2_se: f64) -> RecordBatch {
        let schema = crate::nodes::ldsc_hsq::output_schema();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![h2])),
                Arc::new(Float64Array::from(vec![h2_se])),
                Arc::new(Float64Array::from(vec![1.0])), // intercept
                Arc::new(Float64Array::from(vec![0.01])), // intercept_se
                Arc::new(Float64Array::from(vec![0.1])), // ratio
                Arc::new(Float64Array::from(vec![0.05])), // ratio_se
                Arc::new(Float64Array::from(vec![1.2])), // mean_chisq
                Arc::new(Float64Array::from(vec![1.1])), // lambda_gc
                Arc::new(Float64Array::from(vec![1000.0])), // n_snp
                Arc::new(StringArray::from(vec!["[0.3]"])), // coef
                Arc::new(StringArray::from(vec!["[0.05]"])), // coef_se
            ],
        )
        .unwrap()
    }

    /// `P=0.5, K=0.01` → conversion factor ≈ 0.5519 (matches the
    /// `h2_obs_to_liab_scz` golden value in `ldsc::regress`).
    #[test]
    fn conversion_factor_balanced_sample_of_1pct_phenotype() {
        let node = LiabilityNode::new(LiabilityConfig::new(0.5, 0.01));
        let c = node.conversion_factor().unwrap();
        // Matches the `h2_obs_to_liab_scz` golden value from `ldsc::regress`
        // to within the precision of the ported `norm_isf` / `norm_pdf`.
        assert!((c - 0.551907298063).abs() < 1e-6, "c = {c}");
    }

    /// h² and its SE are both scaled by `c`; every upstream column survives
    /// unchanged; the output schema carries exactly two extra columns.
    #[test]
    fn apply_scales_h2_and_passes_through_other_columns() {
        let h2_obs = 0.3;
        let h2_se_obs = 0.05;
        let node = LiabilityNode::new(LiabilityConfig::new(0.5, 0.01));
        let c = node.conversion_factor().unwrap();

        let out = node.apply(&[h2_summary_batch(h2_obs, h2_se_obs)]).unwrap();
        assert_eq!(out.len(), 1);
        let b = &out[0];
        assert_eq!(b.num_columns(), output_schema().fields().len());
        assert_eq!(b.num_rows(), 1);

        // h2_liab = c · h2_obs, h2_se_liab = c · h2_se_obs.
        let h2_liab = b
            .column(b.schema().index_of(H2_LIAB_COL).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        let h2_se_liab = b
            .column(b.schema().index_of(H2_SE_LIAB_COL).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!((h2_liab - c * h2_obs).abs() < 1e-12, "h2_liab = {h2_liab}");
        assert!(
            (h2_se_liab - c * h2_se_obs).abs() < 1e-12,
            "h2_se_liab = {h2_se_liab}"
        );

        // Observed-scale h2 / h2_se and the χ²-scale columns are untouched.
        assert_eq!(b.schema(), output_schema());
        let h2_passthrough = b
            .column(b.schema().index_of(H2_COL).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!((h2_passthrough - h2_obs).abs() < 1e-12);
        let mean_chisq = b
            .column(b.schema().index_of("mean_chisq").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!((mean_chisq - 1.2).abs() < 1e-12);
    }

    /// Invalid prevalences surface the underlying `LdscError` rather than a
    /// bogus conversion.
    #[test]
    fn invalid_prevalences_error() {
        let node = LiabilityNode::new(LiabilityConfig::new(0.5, 1.0));
        assert!(node.conversion_factor().is_err());
        let node = LiabilityNode::new(LiabilityConfig::new(0.0, 0.01));
        assert!(node.conversion_factor().is_err());
    }

    /// `build_result_batch`-style structural check: the declared port output
    /// schema is exactly the upstream h² schema + the two liability columns,
    /// in order.
    #[test]
    fn output_schema_is_h2_summary_plus_two_liability_cols() {
        let s = output_schema();
        let base = crate::nodes::ldsc_hsq::output_schema();
        assert_eq!(s.fields().len(), base.fields().len() + 2);
        // First len(base) fields identical to the upstream h² summary.
        for (i, f) in base.fields().iter().enumerate() {
            assert_eq!(s.field(i), f.as_ref(), "field {i} drifted");
        }
        let last = s.fields().len() - 1;
        assert_eq!(s.field(last - 1).name(), H2_LIAB_COL);
        assert_eq!(s.field(last).name(), H2_SE_LIAB_COL);
    }
}
