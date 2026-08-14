//! HDL-L DAG node — local genetic correlation analysis for one region.
//!
//! One node = one genomic region (mirroring R's `HDL.L`, which analyses a
//! single `chr`+`piece`). It reads two GWAS sumstat tables, builds the LD
//! reference from a PLINK `.bed/.bim/.fam` prefix, harmonises the sumstats,
//! runs [`hdl::locus::run_locus`], and emits a one-row result table.
//!
//! For genome-wide local-rG scans across many regions, chain one `hdl_l` node
//! per region (or run the `hdl` crate directly and loop).
//!
//! # Region size limit
//!
//! The region width (`stop - start`) **must not exceed 5 Mb** (5,000,000 bp).
//! HDL-L diagonalises an LD matrix whose dimension equals the number of
//! in-region reference SNPs; a larger window makes the eigen-decomposition
//! and likelihood optimisation prohibitively slow and memory-hungry. The
//! check is enforced at execution time in [`HdlLNode::execute`].

use std::path::PathBuf;
use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::dag::runtime::RuntimeStatus;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

const HDL_L_KIND: &str = "hdl_l";

/// Maximum permitted region width (bp). Regions wider than this are rejected
/// because the eigen-decomposition / likelihood optimisation scales poorly
/// with the number of in-region SNPs. See the module-level "Region size
/// limit" note.
pub const MAX_REGION_WIDTH: i64 = 5_000_000;

pub(crate) fn result_schema() -> SchemaRef {
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

/// Hardcoded per-chromosome PLINK reference prefix (EUR 1000G, one `.bed/.bim/.fam`
/// per chromosome). `{N}` is resolved to [`HdlLSpec::chr`] at execution time.
///
/// This is the **same** panel used by [`super::lava::LavaLocusNode`] — see
/// `lava::REF_PREFIX_TEMPLATE`. Both nodes share the reference so results are
/// directly comparable.
pub(crate) const REF_PREFIX_TEMPLATE: &str =
    "/mnt/disk2/dataset/1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc";

/// Spec for [`HdlLNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HdlLSpec {
    /// Chromosome number of the region being analysed (1–22). Used to resolve
    /// the `{N}` placeholder in [`REF_PREFIX_TEMPLATE`].
    pub chr: i64,
    /// Region start (bp, 1-based inclusive).
    ///
    /// The window `stop - start` must be ≤ [`MAX_REGION_WIDTH`] (5 Mb);
    /// wider regions are rejected at execution time.
    pub start: i64,
    /// Region stop (bp, 1-based inclusive).
    ///
    /// The window `stop - start` must be ≤ [`MAX_REGION_WIDTH`] (5 Mb);
    /// wider regions are rejected at execution time.
    pub stop: i64,
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
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(HdlLNode::new(serde_json::from_value(spec)?)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<HdlLSpec>(spec, "hdl_l")?;
        let input1 = ctx
            .input_vars
            .first()
            .cloned()
            .unwrap_or_else(|| "__missing_input_0".into());
        let input2 = ctx
            .input_vars
            .get(1)
            .cloned()
            .unwrap_or_else(|| "__missing_input_1".into());
        let out = ctx.output_var.to_string();
        let code = vec![
            format!("# HDL-L: Local genetic correlation"),
            format!("# Region: chr{}:{}-{}", s.chr, s.start, s.stop),
            format!("# Traits: {} vs {}", s.trait1_name, s.trait2_name),
            format!("# NOTE: HDL requires preprocessed sumstats + LD reference"),
            format!("{out} <- HDL::HDL.analysis("),
            format!("  trait1_sumstats = {input1},"),
            format!("  trait2_sumstats = {input2},"),
            format!("  trait1.name = \"{}\",", s.trait1_name),
            format!("  trait2.name = \"{}\",", s.trait2_name),
            format!("  chr = {},", s.chr),
            format!("  start = {},", s.start),
            format!("  stop = {},", s.stop),
            format!("  n0 = {},", s.n0),
            format!("  nref = {}", s.nref),
            format!(")"),
            format!("print({out})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["HDL".into()]
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

/// Extract string values from a column, accepting both `StringArray` (Utf8)
/// and `StringViewArray` (Utf8View) — see [`dag_core::node::string_opt_values`].
fn col_str(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for v in dag_core::node::string_opt_values(col.as_ref())? {
            out.push(v.unwrap_or_default());
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
/// Shared by [`HdlLNode`] and [`crate::nodes::hdl_l_scan::HdlLScanNode`].
///
/// Z is taken from a `Z`/`STAT`/`Zscore` column if present; otherwise from
/// `b`/`BETA` (or `OR`, log-transformed) divided by `se` — mirroring
/// `HDL.L.R` lines 215-280.
pub(crate) fn parse_sumstats(
    batches: &[RecordBatch],
) -> Result<Vec<hdl::input::SumStatRow>, DagError> {
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

pub(crate) async fn collect_input_batches(
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
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let err = |msg: String| DagError::NodeError {
            node_type: HDL_L_KIND.into(),
            msg,
        };

        // ---- Enforce the region-size guard up front ----
        let width = self.spec.stop - self.spec.start;
        if width < 0 {
            return Err(err(format!(
                "region stop ({}) < start ({})",
                self.spec.stop, self.spec.start
            )));
        }
        if width > MAX_REGION_WIDTH {
            return Err(err(format!(
                "region width {width} bp exceeds the 5 Mb limit ({MAX_REGION_WIDTH} bp); \
                 split into smaller windows"
            )));
        }

        let in0 = inputs.first().ok_or_else(|| err("no GWAS1 input".into()))?;
        let in1 = inputs.get(1).ok_or_else(|| err("no GWAS2 input".into()))?;
        let b1 = collect_input_batches(in0, HDL_L_KIND).await?;
        let b2 = collect_input_batches(in1, HDL_L_KIND).await?;

        // ---- Resolve the per-chromosome PLINK reference prefix ----
        let ref_template =
            REF_PREFIX_TEMPLATE.to_string();
        let ld_ref_prefix = PathBuf::from(ref_template.replace("{N}", &self.spec.chr.to_string()));

        // ---- Filter reference SNPs to the region [start, stop] ----
        // Load the .bim to get SNP ids + positions, keep only those within the
        // region window, then build the LD reference from that subset. This
        // mirrors how LAVA's process.locus extracts a locus from the per-chrom
        // panel — same reference, region-level slice.
        let refr = lava::plink::load_reference(&ld_ref_prefix)
            .map_err(|e| err(format!("loading .bim/.fam: {e}")))?;
        let si = &refr.snp_info;
        let region_snps: Vec<String> = (0..si.snp.len())
            .filter(|&i| {
                si.chr[i] == self.spec.chr
                    && si.pos[i] >= self.spec.start
                    && si.pos[i] <= self.spec.stop
            })
            .map(|i| si.snp[i].clone())
            .collect();
        if region_snps.is_empty() {
            return Err(err(format!(
                "no reference SNPs in chr{}:{}-{}",
                self.spec.chr, self.spec.start, self.spec.stop
            )));
        }
        reporter.info(format!(
            "hdl_l: {} ({} rows) ~ {} ({} rows); loading LD ref {} (chr{}:{}-{}, {} SNPs)",
            self.spec.trait1_name,
            b1.iter().map(|b| b.num_rows()).sum::<usize>(),
            self.spec.trait2_name,
            b2.iter().map(|b| b.num_rows()).sum::<usize>(),
            ld_ref_prefix.display(),
            self.spec.chr,
            self.spec.start,
            self.spec.stop,
            region_snps.len(),
        ));

        // ---- LD reference from PLINK (region-filtered) ----
        let ldref = hdl::reference::ld_ref_from_plink(&ld_ref_prefix, &region_snps)
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
        let ctx = dag_core::registry::new_isolated_ctx(
            node_ctx.runtime_env.clone(),
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

// =====================================================================
// Integration test — exercises the full node against the real 1000G EUR
// chr22 panel. Ignored by default (needs /mnt/disk2/dataset/1000g_plink).
// Run with:
//   cargo test -p data-engine -- --ignored hdl_l_e2e_real_panel
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }

    /// Read SNP / A1 / A2 from a .bim file for SNPs within [start, stop].
    fn read_region_bim(
        prefix: &str,
        chr: i64,
        start: i64,
        stop: i64,
    ) -> Vec<(String, String, String)> {
        let bim = std::fs::read_to_string(format!("{prefix}.bim")).unwrap();
        bim.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 6 {
                    return None;
                }
                let c: i64 = f[0].parse().ok()?;
                let pos: i64 = f[3].parse().ok()?;
                if c == chr && pos >= start && pos <= stop {
                    Some((f[1].to_string(), f[4].to_string(), f[5].to_string()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Build a sumstat RecordBatch from (snp, a1, a2) tuples with synthetic Z + N.
    fn make_sumstats(snps: &[(String, String, String)], z_seed: f64, n: f64) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("SNP", DataType::Utf8, false),
            Field::new("A1", DataType::Utf8, false),
            Field::new("A2", DataType::Utf8, false),
            Field::new("Z", DataType::Float64, false),
            Field::new("N", DataType::Float64, false),
        ]));
        let snp_arr: Vec<&str> = snps.iter().map(|(s, _, _)| s.as_str()).collect();
        let a1_arr: Vec<&str> = snps.iter().map(|(_, a, _)| a.as_str()).collect();
        let a2_arr: Vec<&str> = snps.iter().map(|(_, _, a)| a.as_str()).collect();
        let z_arr: Vec<f64> = (0..snps.len())
            .map(|i| {
                // deterministic pseudo-Z: alternate sign, decreasing magnitude
                let z = z_seed * (1.0 - (i as f64 / snps.len() as f64));
                if i % 2 == 0 { z } else { -z }
            })
            .collect();
        let n_arr = vec![n; snps.len()];
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(snp_arr)),
                Arc::new(StringArray::from(a1_arr)),
                Arc::new(StringArray::from(a2_arr)),
                Arc::new(Float64Array::from(z_arr)),
                Arc::new(Float64Array::from(n_arr)),
            ],
        )
        .unwrap()
    }

    /// End-to-end: HDL-L node reads the real 1000G EUR chr22 panel,
    /// filters to a ~472-SNP region, builds the LD reference, and runs the
    /// full MLE + LRT pipeline.
    #[tokio::test]
    #[ignore = "needs local 1000G EUR PLINK panel at /mnt/disk2/dataset/1000g_plink"]
    async fn hdl_l_e2e_real_panel() {
        let chr = 22i64;
        let start = 17_000_000i64;
        let stop = 17_100_000i64;
        let prefix = REF_PREFIX_TEMPLATE.replace("{N}", &chr.to_string());

        // Read region SNPs from the .bim
        let snps = read_region_bim(&prefix, chr, start, stop);
        assert!(
            snps.len() > 100,
            "expected >100 SNPs in region, got {}",
            snps.len()
        );
        eprintln!("region chr{chr}:{start}-{stop}: {} SNPs", snps.len());

        // Build synthetic sumstats for two "traits" with slightly different Z profiles
        let ctx = node_ctx();
        let batch1 = make_sumstats(&snps, 2.5, 50_000.0);
        let batch2 = make_sumstats(&snps, 1.8, 80_000.0);
        let sess = SessionContext::new();
        let df1 = sess.read_batch(batch1).unwrap();
        let df2 = sess.read_batch(batch2).unwrap();

        let mut node = HdlLNode::new(HdlLSpec {
            chr,
            start,
            stop,
            trait1_name: "traitA".into(),
            trait2_name: "traitB".into(),
            n0: 0.0,
            nref: default_nref(),
            eigen_cut: default_eigen_cut(),
            alpha: default_alpha(),
        });

        let reporter = dag_core::dag::node_event::NodeReporter::noop();
        let res = node
            .execute(
                &ctx,
                &[
                    NodeInput { port: 0, data: df1 },
                    NodeInput { port: 0, data: df2 },
                ],
                &reporter,
            )
            .await
            .expect("HDL-L execute should succeed");

        let df = &res[&0];
        let batches = df.clone().collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);

        // Verify output columns
        let row = &batches[0];
        let trait1 = col_str(std::slice::from_ref(row), "trait1").unwrap()[0].clone();
        let trait2 = col_str(std::slice::from_ref(row), "trait2").unwrap()[0].clone();
        assert_eq!(trait1, "traitA");
        assert_eq!(trait2, "traitB");

        let n_retained_arr = row.column_by_name("n_retained").unwrap();
        let n_retained = n_retained_arr
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0);
        eprintln!("n_retained = {n_retained}");
        assert!(
            n_retained > 50,
            "should retain a meaningful number of eigen-components"
        );

        // h² estimates — synthetic data may yield h² ≤ 0, in which case HDL
        // intentionally returns NaN for rg/p_h12 (the genetic covariance is
        // undefined when either trait has no signal). Verify structural validity.
        let h11 = col_f64(std::slice::from_ref(row), "h11").unwrap()[0];
        let h22 = col_f64(std::slice::from_ref(row), "h22").unwrap()[0];
        let rg = col_f64(std::slice::from_ref(row), "rg").unwrap()[0];
        eprintln!("h11={h11:.4}, h22={h22:.4}, rg={rg:.4}");

        if h11 > 0.0 && h22 > 0.0 {
            assert!(
                (-1.01..=1.01).contains(&rg),
                "rg should be in [-1, 1], got {rg}"
            );
            let p_h12 = col_f64(std::slice::from_ref(row), "p_h12").unwrap()[0];
            eprintln!("p_h12 = {p_h12:.3e}");
            assert!(
                (0.0..=1.0).contains(&p_h12),
                "p-value out of range: {p_h12}"
            );
        } else {
            eprintln!("(h² ≤ 0 for at least one trait — rg/p_h12 are NaN by design)");
            assert!(rg.is_nan(), "rg should be NaN when h² ≤ 0");
        }

        let converged = row.column_by_name("converged").unwrap();
        let conv_arr = converged.as_any().downcast_ref::<BooleanArray>().unwrap();
        assert!(conv_arr.value(0), "estimation should converge");

        eprintln!(
            "✅ HDL-L e2e real panel: n_retained={n_retained}, h11={h11:.4}, h22={h22:.4}, rg={rg:.4}, converged=true"
        );
    }
}
