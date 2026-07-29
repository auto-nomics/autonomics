//! HDL-L DAG node — local genetic correlation analysis for one region.
//!
//! One node = one genomic region (mirroring R's `HDL.L`, which analyses a
//! single `chr`+`piece`). It reads two GWAS sumstat tables, builds the LD
//! reference from a PLINK `.bed/.bim/.fam` prefix, harmonises the sumstats,
//! runs [`hdl::locus::run_locus`], and emits a one-row result table.
//!
//! For genome-wide local-rG scans across many regions, chain one `hdl_l` node
//! per region (or run the `hdl` crate directly and loop).

use std::path::PathBuf;
use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::dag::runtime::RuntimeStatus;
use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

const HDL_L_KIND: &str = "hdl_l";

fn result_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("trait1", DataType::Utf8, false),
        Field::new("trait2", DataType::Utf8, false),
        Field::new("h11", DataType::Float64, true),
        Field::new("h22", DataType::Float64, true),
        Field::new("h12", DataType::Float64, true),
        Field::new("rg", DataType::Float64, true),
        Field::new("rg_lower", DataType::Float64, true),
        Field::new("rg_upper", DataType::Float64, true),
        Field::new("p_h1", DataType::Float64, true),
        Field::new("p_h2", DataType::Float64, true),
        Field::new("p_h12", DataType::Float64, true),
        Field::new("int_h11", DataType::Float64, true),
        Field::new("int_h22", DataType::Float64, true),
        Field::new("int_h12", DataType::Float64, true),
        Field::new("n_retained", DataType::Int64, false),
        Field::new("converged", DataType::Boolean, false),
    ]))
}

/// Spec for [`HdlLNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HdlLSpec {
    /// PLINK `.bed/.bim/.fam` prefix for this region's LD reference.
    pub ld_ref_prefix: String,
    pub trait1_name: String,
    pub trait2_name: String,
    /// Sample overlap (0 for independent cohorts; HDL-L default).
    #[serde(default)]
    pub n0: f64,
    /// LD-reference sample size (UKB default 335,272).
    #[serde(default = "default_nref")]
    pub nref: f64,
    /// Eigen-cut cumulative-variance threshold (default 0.99).
    #[serde(default = "default_eigen_cut")]
    pub eigen_cut: f64,
    /// Significance level for the likelihood-based CI (default 0.05 → 95% CI).
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}
fn default_nref() -> f64 {
    hdl::locus::DEFAULT_NREF
}
fn default_eigen_cut() -> f64 {
    hdl::locus::DEFAULT_EIGEN_CUT
}
fn default_alpha() -> f64 {
    hdl::locus::DEFAULT_ALPHA
}

#[derive(Clone)]
pub struct HdlLNode {
    meta: NodePorts,
    spec: HdlLSpec,
}

impl HdlLNode {
    pub fn new(spec: HdlLSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_input_port(None)
                .add_output_port(Some(result_schema())),
            spec,
        }
    }
}

pub struct HdlLNodeFactory {}

impl NodeFactory for HdlLNodeFactory {
    fn kind(&self) -> &'static str {
        HDL_L_KIND
    }
    fn desc(&self) -> &'static str {
        "HDL-L local genetic correlation (one region)."
    }
    fn doc(&self) -> &'static str {
        "Reads two GWAS sumstat tables + a PLINK LD reference for one region, \
         runs the HDL-L MLE (full-likelihood h² + conditional genetic covariance) \
         with LRT P values and profile-likelihood CIs."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HdlLSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(Some(result_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(HdlLNode::new(serde_json::from_value(spec)?)))
    }
}

// ---- arrow column helpers (local; match lava::nodes conventions) ----

fn arr_f64(arr: &dyn Array, i: usize) -> f64 {
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
        return a.value(i);
    }
    if let Some(a) = arr.as_any().downcast_ref::<Float32Array>() {
        return a.value(i) as f64;
    }
    f64::NAN
}

fn col_str(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<StringArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                String::new()
            } else {
                arr.value(i).to_string()
            });
        }
    }
    Some(out)
}

fn col_f64(batches: &[RecordBatch], name: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for i in 0..col.len() {
            out.push(if col.is_null(i) {
                f64::NAN
            } else {
                arr_f64(col.as_ref(), i)
            });
        }
    }
    Some(out)
}

/// Build [`hdl::input::SumStatRow`]s from a sumstat RecordBatch.
///
/// Z is taken from a `Z`/`STAT`/`Zscore` column if present; otherwise from
/// `b`/`BETA` (or `OR`, log-transformed) divided by `se` — mirroring
/// `HDL.L.R` lines 215-280.
fn parse_sumstats(batches: &[RecordBatch]) -> Result<Vec<hdl::input::SumStatRow>, DagError> {
    let snp = col_str(batches, "SNP")
        .or_else(|| col_str(batches, "snp"))
        .or_else(|| col_str(batches, "rsid"))
        .ok_or_else(|| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg: "sumstats missing SNP column".into(),
        })?;
    let a1 = col_str(batches, "A1")
        .or_else(|| col_str(batches, "a1"))
        .unwrap_or_default();
    let a2 = col_str(batches, "A2")
        .or_else(|| col_str(batches, "a2"))
        .unwrap_or_default();
    let n = col_f64(batches, "N")
        .or_else(|| col_f64(batches, "n"))
        .unwrap_or_else(|| vec![f64::NAN; snp.len()]);

    let z = if let Some(z) = col_f64(batches, "Z").or_else(|| col_f64(batches, "STAT")) {
        z
    } else {
        // derive from b + se
        let b = col_f64(batches, "b")
            .or_else(|| col_f64(batches, "BETA"))
            .or_else(|| col_f64(batches, "OR"))
            .ok_or_else(|| DagError::NodeError {
                node_type: HDL_L_KIND.into(),
                msg: "sumstats missing Z (or b/BETA + se)".into(),
            })?;
        let se = col_f64(batches, "se").ok_or_else(|| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg: "sumstats missing se".into(),
        })?;
        // OR → log(OR); plain b stays. Detect OR by median(|b|) ≈ 1.
        let med = {
            let mut s: Vec<f64> = b
                .iter()
                .filter(|v| v.is_finite())
                .map(|v| v.abs())
                .collect();
            s.sort_by(|x, y| x.partial_cmp(y).unwrap());
            s.get(s.len() / 2).copied().unwrap_or(0.0)
        };
        b.iter()
            .zip(&se)
            .map(|(bv, sv)| {
                let eff = if (med - 1.0).abs() < 0.1 {
                    bv.ln()
                } else {
                    *bv
                };
                if sv.is_finite() && sv.abs() > 0.0 {
                    eff / sv
                } else {
                    f64::NAN
                }
            })
            .collect()
    };

    let mut rows = Vec::with_capacity(snp.len());
    for i in 0..snp.len() {
        if snp[i].is_empty() || z[i].is_nan() || n[i].is_nan() || n[i] <= 0.0 {
            continue;
        }
        rows.push(hdl::input::SumStatRow {
            snp: snp[i].clone(),
            a1: a1.get(i).cloned().unwrap_or_default(),
            a2: a2.get(i).cloned().unwrap_or_default(),
            n: n[i],
            z: z[i],
        });
    }
    Ok(rows)
}

async fn collect_input_batches(
    input: &NodeInput,
    kind: &str,
) -> Result<Vec<RecordBatch>, DagError> {
    let batches: Vec<RecordBatch> =
        input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect failed: {e}"),
            })?;
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(DagError::NodeError {
            node_type: kind.into(),
            msg: "empty input".into(),
        });
    }
    Ok(batches)
}

#[async_trait]
impl DagNode for HdlLNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        HDL_L_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let err = |msg: String| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg,
        };

        let in0 = inputs.first().ok_or_else(|| err("no GWAS1 input".into()))?;
        let in1 = inputs.get(1).ok_or_else(|| err("no GWAS2 input".into()))?;
        let b1 = collect_input_batches(in0, HDL_L_KIND).await?;
        let b2 = collect_input_batches(in1, HDL_L_KIND).await?;

        reporter.info(format!(
            "hdl_l: {} ({} rows) ~ {} ({} rows); loading LD ref {}",
            self.spec.trait1_name,
            b1.iter().map(|b| b.num_rows()).sum::<usize>(),
            self.spec.trait2_name,
            b2.iter().map(|b| b.num_rows()).sum::<usize>(),
            self.spec.ld_ref_prefix,
        ));

        // ---- LD reference from PLINK ----
        let ldref = hdl::reference::ld_ref_from_plink_all(&PathBuf::from(&self.spec.ld_ref_prefix))
            .map_err(|e| err(format!("LD reference: {e}")))?;

        // ---- harmonise sumstats → bhat ----
        let rows1 = parse_sumstats(&b1)?;
        let rows2 = parse_sumstats(&b2)?;
        let (bhat1, n1) = hdl::input::harmonise_gwas(&rows1, &ldref.snps, &ldref.a2_ref)
            .map_err(|e| err(e.to_string()))?;
        let (bhat2, n2) = hdl::input::harmonise_gwas(&rows2, &ldref.snps, &ldref.a2_ref)
            .map_err(|e| err(e.to_string()))?;

        reporter.info(format!(
            "hdl_l: region has {} reference SNPs; estimating",
            ldref.lam.len()
        ));

        let lim = hdl::locus::DEFAULT_LIM;
        let res = hdl::locus::run_locus(
            &bhat1,
            &bhat2,
            &ldref.lam,
            ldref.v.as_ref(),
            &ldref.ldsc,
            n1,
            n2,
            self.spec.n0,
            self.spec.nref,
            self.spec.eigen_cut,
            lim,
            self.spec.alpha,
        )
        .map_err(|e| err(format!("HDL-L estimation: {e}")))?;

        reporter.info(format!(
            "hdl_l: done — rg={:.4} [ {:.4}, {:.4} ] p_h12={:.3e}",
            res.rg, res.rg_lower, res.rg_upper, res.p_h12
        ));

        let f = |x: f64| if x.is_nan() { None } else { Some(x) };
        let batch = RecordBatch::try_new(
            result_schema(),
            vec![
                Arc::new(StringArray::from(vec![self.spec.trait1_name.clone()])),
                Arc::new(StringArray::from(vec![self.spec.trait2_name.clone()])),
                Arc::new(Float64Array::from(vec![f(res.h11)])),
                Arc::new(Float64Array::from(vec![f(res.h22)])),
                Arc::new(Float64Array::from(vec![f(res.h12)])),
                Arc::new(Float64Array::from(vec![f(res.rg)])),
                Arc::new(Float64Array::from(vec![f(res.rg_lower)])),
                Arc::new(Float64Array::from(vec![f(res.rg_upper)])),
                Arc::new(Float64Array::from(vec![f(res.p_h1)])),
                Arc::new(Float64Array::from(vec![f(res.p_h2)])),
                Arc::new(Float64Array::from(vec![f(res.p_h12)])),
                Arc::new(Float64Array::from(vec![f(res.int_h11)])),
                Arc::new(Float64Array::from(vec![f(res.int_h22)])),
                Arc::new(Float64Array::from(vec![f(res.int_h12)])),
                Arc::new(Int64Array::from(vec![res.n_retained as i64])),
                Arc::new(BooleanArray::from(vec![res.converged])),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg: format!("arrow: {e}"),
        })?;

        // wrap into a PortOutputs via an isolated SessionContext (like lava nodes)
        let ctx = crate::node_registry::registry::new_isolated_ctx(
            node_ctx.runtime_env.clone(),
            node_ctx.iceberg_catalog.clone(),
        );
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg: format!("read_batch: {e}"),
        })?;
        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, df);
        Ok(out)
    }
}
