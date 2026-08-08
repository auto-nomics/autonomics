//! HDL-L scan node — genome-wide (or chromosome-wide) local genetic
//! correlation scanning across many adjacent genomic windows.
//!
//! One node replaces an entire batch of [`crate::hdl_l::HdlLNode`] instances for
//! scanning. It loads the per-chromosome PLINK `.bim/.fam` **once** (vs. once
//! per window for the single-region node), parses both GWAS sumstat inputs
//! **once**, then iterates the user-defined windows. Each window is processed
//! independently with per-window error isolation: a window with too few SNPs,
//! a failed eigen-decomposition, or a non-convergent MLE emits a result row
//! with `converged = false` / NaN estimates and a diagnostic `error` string,
//! rather than aborting the entire scan.
//!
//! # Window definition
//!
//! Windows are defined in **range mode**: a uniform tiling of
//! `[scan_start, scan_stop]` by `window_size` (stepped by `step`). Optionally
//! `skip_ranges` (e.g. the MHC region) excludes windows whose midpoint falls
//! inside a listed range. Each window's width must be ≤ 5 Mb (the same
//! [`MAX_REGION_WIDTH`] guard as the single-region node).
//!
//! # Concurrency
//!
//! `concurrency` controls how many windows run in parallel via
//! `tokio::task::spawn_blocking`. The default is 1 (sequential), which lets
//! faer's internal multithreading use all cores per window without
//! oversubscription. Increase it for many small windows where a single
//! eigen-decomposition does not saturate the CPU.

use std::path::PathBuf;
use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::hdl_l::{
    MAX_REGION_WIDTH, REF_PREFIX_TEMPLATE, collect_input_batches, parse_sumstats, result_schema,
};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::dag::runtime::RuntimeStatus;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

const HDL_L_SCAN_KIND: &str = "hdl_l_scan";

// =====================================================================
// Spec
// =====================================================================

/// Spec for [`HdlLScanNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HdlLScanSpec {
    /// Chromosome number (1–22). The PLINK reference is resolved via the
    /// `{N}` template in [`REF_PREFIX_TEMPLATE`].
    pub chr: i64,
    /// Start of the scan range (bp, 1-based inclusive).
    pub scan_start: i64,
    /// End of the scan range (bp, 1-based inclusive).
    pub scan_stop: i64,
    /// Window width (bp). Each window is `[start, start + window_size - 1]`.
    /// Must be ≤ 5 Mb (same limit as the single-region node).
    pub window_size: i64,
    /// Step between consecutive window starts (bp). When `step == window_size`
    /// the windows are non-overlapping tiles; `step < window_size` produces
    /// overlapping windows.
    pub step: i64,
    pub trait1_name: String,
    pub trait2_name: String,
    /// Sample overlap (0 for independent cohorts).
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
    /// Minimum number of reference SNPs required in a window to attempt
    /// estimation. Windows with fewer SNPs are skipped (emitted as a row
    /// with `converged = false`). Default 20.
    #[serde(default = "default_min_snps")]
    pub min_snps: usize,
    /// Regions to skip, as `[start, stop]` bp pairs. A window is skipped if
    /// its **midpoint** falls inside any skip range. Typical use: the MHC
    /// region `[[25000000, 35000000]]` on chr6.
    #[serde(default)]
    pub skip_ranges: Vec<[i64; 2]>,
    /// How many windows to process in parallel via `spawn_blocking`. Default
    /// 1 (sequential, lets faer use all cores per window).
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
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
fn default_min_snps() -> usize {
    20
}
fn default_concurrency() -> usize {
    1
}

/// Output schema: the single-region result columns + scan-position columns.
fn scan_result_schema() -> SchemaRef {
    let mut fields = vec![
        Field::new("chr", DataType::Int64, false),
        Field::new("start", DataType::Int64, false),
        Field::new("stop", DataType::Int64, false),
        Field::new("n_ref_snps", DataType::Int64, false),
    ];
    // Reuse the single-region result fields (trait1, trait2, h11, …, converged).
    fields.extend(result_schema().fields().iter().map(|f| f.as_ref().clone()));
    fields.push(Field::new("error", DataType::Utf8, true));
    Arc::new(Schema::new(fields))
}

// =====================================================================
// Window expansion
// =====================================================================

/// A concrete, expanded window ready for estimation.
#[derive(Clone, Copy)]
struct Window {
    start: i64,
    stop: i64,
}

/// Expand `scan_start..=scan_stop` into windows of `window_size` stepped by
/// `step`, skipping windows whose midpoint falls inside any `skip_ranges`.
fn expand_windows(spec: &HdlLScanSpec) -> Vec<Window> {
    let mut out = Vec::new();
    let mut start = spec.scan_start;
    while start <= spec.scan_stop {
        let stop = (start + spec.window_size - 1).min(spec.scan_stop);
        let mid = start + (stop - start) / 2;
        let in_skip = spec.skip_ranges.iter().any(|r| mid >= r[0] && mid <= r[1]);
        if !in_skip {
            out.push(Window { start, stop });
        }
        start += spec.step;
    }
    out
}

// =====================================================================
// Per-window result
// =====================================================================

/// Outcome of processing one window — either a successful [`LocusResult`]
/// or a diagnostic error string.
struct WindowOutcome {
    window: Window,
    n_ref_snps: i64,
    result: Result<hdl::locus::LocusResult, String>,
}

/// Process a single window: filter reference SNPs to the region, build the
/// eigen LD reference, harmonise sumstats, and run the HDL-L estimation.
///
/// This is the CPU-heavy inner loop. It is designed to be called inside
/// `spawn_blocking`.
fn process_window(
    window: Window,
    chr: i64,
    prefix: &PathBuf,
    plink_ref: &lava::input::PlinkRef,
    rows1: &[hdl::input::SumStatRow],
    rows2: &[hdl::input::SumStatRow],
    spec: &HdlLScanSpec,
) -> WindowOutcome {
    // ---- Filter reference SNPs to the region ----
    let si = &plink_ref.snp_info;
    let region_snps: Vec<String> = (0..si.snp.len())
        .filter(|&i| si.chr[i] == chr && si.pos[i] >= window.start && si.pos[i] <= window.stop)
        .map(|i| si.snp[i].clone())
        .collect();
    let n_ref_snps = region_snps.len() as i64;

    if (n_ref_snps as usize) < spec.min_snps {
        return WindowOutcome {
            window,
            n_ref_snps,
            result: Err(format!(
                "only {n_ref_snps} reference SNPs (< min_snps {}); skipped",
                spec.min_snps
            )),
        };
    }

    // ---- Build eigen LD reference for the region ----
    let ldref = match hdl::reference::ld_ref_from_plink_with_ref(prefix, plink_ref, &region_snps) {
        Ok(r) => r,
        Err(e) => {
            return WindowOutcome {
                window,
                n_ref_snps,
                result: Err(format!("LD reference: {e}")),
            };
        }
    };

    // ---- Harmonise sumstats → bhat ----
    let (bhat1, n1) = match hdl::input::harmonise_gwas(rows1, &ldref.snps, &ldref.a2_ref) {
        Ok(v) => v,
        Err(e) => {
            return WindowOutcome {
                window,
                n_ref_snps,
                result: Err(format!("harmonise trait1: {e}")),
            };
        }
    };
    let (bhat2, n2) = match hdl::input::harmonise_gwas(rows2, &ldref.snps, &ldref.a2_ref) {
        Ok(v) => v,
        Err(e) => {
            return WindowOutcome {
                window,
                n_ref_snps,
                result: Err(format!("harmonise trait2: {e}")),
            };
        }
    };

    // ---- Run HDL-L estimation ----
    let lim = hdl::locus::DEFAULT_LIM;
    let res = hdl::locus::run_locus(
        &bhat1,
        &bhat2,
        &ldref.lam,
        ldref.v.as_ref(),
        &ldref.ldsc,
        n1,
        n2,
        spec.n0,
        spec.nref,
        spec.eigen_cut,
        lim,
        spec.alpha,
    );
    WindowOutcome {
        window,
        n_ref_snps,
        result: res.map_err(|e| format!("HDL-L estimation: {e}")),
    }
}

// =====================================================================
// Node
// =====================================================================

#[derive(Clone)]
pub struct HdlLScanNode {
    meta: NodePorts,
    spec: HdlLScanSpec,
}

impl HdlLScanNode {
    pub fn new(spec: HdlLScanSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_input_port(None)
                .add_output_port(Some(scan_result_schema())),
            spec,
        }
    }
}

pub struct HdlLScanNodeFactory {}

impl NodeFactory for HdlLScanNodeFactory {
    fn kind(&self) -> &'static str {
        HDL_L_SCAN_KIND
    }
    fn desc(&self) -> &'static str {
        "HDL-L local genetic correlation scan (many windows, one chromosome)."
    }
    fn doc(&self) -> &'static str {
        "Reads two GWAS sumstat tables + a PLINK LD reference, scans across \
         adjacent genomic windows on one chromosome, and runs the HDL-L MLE \
         per window. Loads the .bim once and parses sumstats once (vs. \
         chaining many hdl_l nodes). Emits a combined result table — one row \
         per window — with per-window error isolation (failed windows get \
         NaN estimates + an error string, not an abort)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HdlLScanSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(Some(scan_result_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(HdlLScanNode::new(serde_json::from_value(spec)?)))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<HdlLScanSpec>(spec, "hdl_l_scan")?;
        let input1 = ctx
            .input_vars.first()
            .cloned()
            .unwrap_or_else(|| "__missing_input_0".into());
        let input2 = ctx
            .input_vars
            .get(1)
            .cloned()
            .unwrap_or_else(|| "__missing_input_1".into());
        let out = ctx.output_var.to_string();
        let n_windows = (s.scan_stop - s.scan_start) / s.step.max(1);
        let code = vec![
            format!(
                "# HDL-L Scan: local rg across chr{}:{}-{} (window={}, step={})",
                s.chr, s.scan_start, s.scan_stop, s.window_size, s.step
            ),
            format!(
                "# ~{} windows; traits: {} vs {}",
                n_windows, s.trait1_name, s.trait2_name
            ),
            format!("{out} <- list()"),
            format!(
                "for (start_pos in seq({}, {}, {})) {{",
                s.scan_start, s.scan_stop, s.step
            ),
            format!("  stop_pos <- start_pos + {}", s.window_size),
            format!("  res <- HDL::HDL.analysis("),
            format!("    trait1_sumstats = {input1},"),
            format!("    trait2_sumstats = {input2},"),
            format!("    trait1.name = \"{}\",", s.trait1_name),
            format!("    trait2.name = \"{}\",", s.trait2_name),
            format!("    chr = {},", s.chr),
            format!("    start = start_pos,"),
            format!("    stop = stop_pos,"),
            format!("    n0 = {},", s.n0),
            format!("    nref = {}", s.nref),
            format!("  )"),
            format!(
                "  {out}[[length({out}) + 1]] <- c(start = start_pos, stop = stop_pos, rg = res$rg, p = res$p)"
            ),
            format!("}}"),
            format!("{out} <- do.call(rbind, {out})"),
            format!("print(head({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["HDL".into()]
    }
}

#[async_trait]
impl DagNode for HdlLScanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        HDL_L_SCAN_KIND
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
            node_type: HDL_L_SCAN_KIND.into(),
            msg,
        };

        // ---- Validate window params ----
        if self.spec.window_size <= 0 {
            return Err(err("window_size must be positive".into()));
        }
        if self.spec.step <= 0 {
            return Err(err("step must be positive".into()));
        }
        if self.spec.scan_stop < self.spec.scan_start {
            return Err(err("scan_stop < scan_start".into()));
        }
        if self.spec.window_size > MAX_REGION_WIDTH {
            return Err(err(format!(
                "window_size {} bp exceeds the 5 Mb limit ({MAX_REGION_WIDTH} bp)",
                self.spec.window_size
            )));
        }

        // ---- Expand windows ----
        let windows = expand_windows(&self.spec);
        if windows.is_empty() {
            return Err(err("no windows in the requested scan range".into()));
        }
        let total = windows.len() as u64;
        reporter.info(format!(
            "hdl_l_scan: chr{}: {}-{}, {} windows ({} bp each, step {}), {} skipped",
            self.spec.chr,
            self.spec.scan_start,
            self.spec.scan_stop,
            total,
            self.spec.window_size,
            self.spec.step,
            self.spec.skip_ranges.len(),
        ));

        // ---- Load PLINK reference ONCE ----
        let ld_ref_prefix =
            PathBuf::from(REF_PREFIX_TEMPLATE.replace("{N}", &self.spec.chr.to_string()));
        let plink_ref = lava::plink::load_reference(&ld_ref_prefix)
            .map_err(|e| err(format!("loading .bim/.fam: {e}")))?;
        let plink_ref = Arc::new(plink_ref);
        reporter.info(format!(
            "hdl_l_scan: loaded {} SNPs from {}",
            plink_ref.snp_info.snp.len(),
            ld_ref_prefix.display(),
        ));

        // ---- Collect + parse sumstats ONCE ----
        let in0 = inputs.first().ok_or_else(|| err("no GWAS1 input".into()))?;
        let in1 = inputs.get(1).ok_or_else(|| err("no GWAS2 input".into()))?;
        let b1 = collect_input_batches(in0, HDL_L_SCAN_KIND).await?;
        let b2 = collect_input_batches(in1, HDL_L_SCAN_KIND).await?;
        let rows1 = parse_sumstats(&b1)?;
        let rows2 = parse_sumstats(&b2)?;
        reporter.info(format!(
            "hdl_l_scan: parsed {} trait1 rows, {} trait2 rows",
            rows1.len(),
            rows2.len(),
        ));

        // ---- Process windows ----
        let concurrency = self.spec.concurrency.max(1);
        let sem = Arc::new(Semaphore::new(concurrency));
        let prefix = Arc::new(ld_ref_prefix);
        let spec = Arc::new(self.spec.clone());

        // Wrap sumstats in Arc ONCE — per-window tasks share by reference count,
        // not by deep-cloning the entire chromosome's data for each window
        // (which would be O(N_windows × chr_sumstat_size) memory and the cause
        // of the original OOM).
        let rows1 = Arc::new(rows1);
        let rows2 = Arc::new(rows2);

        let mut futures: FuturesUnordered<
            tokio::task::JoinHandle<Result<(usize, WindowOutcome), String>>,
        > = FuturesUnordered::new();
        for (i, window) in windows.into_iter().enumerate() {
            let sem = Arc::clone(&sem);
            let prefix = Arc::clone(&prefix);
            let plink_ref = Arc::clone(&plink_ref);
            let rows1 = Arc::clone(&rows1);
            let rows2 = Arc::clone(&rows2);
            let spec = Arc::clone(&spec);

            futures.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.map_err(|e| format!("semaphore: {e}"))?;
                // CPU-heavy work in a blocking thread so it doesn't stall
                // the tokio executor.
                let outcome = tokio::task::spawn_blocking(move || {
                    process_window(window, spec.chr, &prefix, &plink_ref, &rows1, &rows2, &spec)
                })
                .await
                .map_err(|e| format!("worker panic: {e}"))?;
                Ok((i, outcome))
            }));
        }

        // ---- Collect outcomes (in completion order) + report progress ----
        let mut outcomes: Vec<Option<WindowOutcome>> = (0..total as usize).map(|_| None).collect();
        let mut done = 0u64;
        let mut errors = 0usize;
        while let Some(res) = futures.next().await {
            match res {
                Ok(Ok((i, outcome))) => {
                    if outcome.result.is_err() {
                        errors += 1;
                    }
                    outcomes[i] = Some(outcome);
                }
                Ok(Err(msg)) => {
                    reporter.info(format!("hdl_l_scan: window task error: {msg}"));
                    errors += 1;
                }
                Err(e) => {
                    reporter.info(format!("hdl_l_scan: window task join failed: {e}"));
                    errors += 1;
                }
            }
            done += 1;
            reporter.progress(done, total);
        }

        // ---- Build combined output table ----
        let outcomes: Vec<WindowOutcome> = outcomes.into_iter().flatten().collect();
        reporter.info(format!(
            "hdl_l_scan: {} windows processed, {} with errors",
            outcomes.len(),
            errors,
        ));

        let batch = build_result_batch(&outcomes, &self.spec)?;
        let ctx = dag_core::registry::new_isolated_ctx(
            node_ctx.runtime_env.clone(),
            node_ctx.iceberg_catalog.clone(),
        );
        let df = ctx.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: HDL_L_SCAN_KIND.into(),
            msg: format!("read_batch: {e}"),
        })?;
        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, df);
        Ok(out)
    }
}

// =====================================================================
// Output builder
// =====================================================================

fn build_result_batch(
    outcomes: &[WindowOutcome],
    spec: &HdlLScanSpec,
) -> Result<RecordBatch, DagError> {
    let n = outcomes.len();
    let schema = scan_result_schema();

    let mut chr_v = Vec::with_capacity(n);
    let mut start_v = Vec::with_capacity(n);
    let mut stop_v = Vec::with_capacity(n);
    let mut n_ref_snps_v = Vec::with_capacity(n);

    // Single-region result columns (from result_schema()).
    let mut trait1_v: Vec<String> = Vec::with_capacity(n);
    let mut trait2_v: Vec<String> = Vec::with_capacity(n);
    let mut h11_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut h22_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut h12_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut rg_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut rg_lower_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut rg_upper_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut p_h1_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut p_h2_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut p_h12_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut int_h11_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut int_h22_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut int_h12_v: Vec<Option<f64>> = Vec::with_capacity(n);
    let mut n_retained_v: Vec<i64> = Vec::with_capacity(n);
    let mut converged_v: Vec<bool> = Vec::with_capacity(n);

    let mut error_v: Vec<Option<String>> = Vec::with_capacity(n);

    let f = |x: f64| if x.is_nan() { None } else { Some(x) };

    for oc in outcomes {
        chr_v.push(spec.chr);
        start_v.push(oc.window.start);
        stop_v.push(oc.window.stop);
        n_ref_snps_v.push(oc.n_ref_snps);

        trait1_v.push(spec.trait1_name.clone());
        trait2_v.push(spec.trait2_name.clone());

        match &oc.result {
            Ok(r) => {
                h11_v.push(f(r.h11));
                h22_v.push(f(r.h22));
                h12_v.push(f(r.h12));
                rg_v.push(f(r.rg));
                rg_lower_v.push(f(r.rg_lower));
                rg_upper_v.push(f(r.rg_upper));
                p_h1_v.push(f(r.p_h1));
                p_h2_v.push(f(r.p_h2));
                p_h12_v.push(f(r.p_h12));
                int_h11_v.push(f(r.int_h11));
                int_h22_v.push(f(r.int_h22));
                int_h12_v.push(f(r.int_h12));
                n_retained_v.push(r.n_retained as i64);
                converged_v.push(r.converged);
                error_v.push(None);
            }
            Err(msg) => {
                for v in [
                    &mut h11_v,
                    &mut h22_v,
                    &mut h12_v,
                    &mut rg_v,
                    &mut rg_lower_v,
                    &mut rg_upper_v,
                    &mut p_h1_v,
                    &mut p_h2_v,
                    &mut p_h12_v,
                    &mut int_h11_v,
                    &mut int_h22_v,
                    &mut int_h12_v,
                ] {
                    v.push(None);
                }
                n_retained_v.push(0);
                converged_v.push(false);
                error_v.push(Some(msg.clone()));
            }
        }
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(chr_v)),
            Arc::new(Int64Array::from(start_v)),
            Arc::new(Int64Array::from(stop_v)),
            Arc::new(Int64Array::from(n_ref_snps_v)),
            Arc::new(StringArray::from(trait1_v)),
            Arc::new(StringArray::from(trait2_v)),
            Arc::new(Float64Array::from(h11_v)),
            Arc::new(Float64Array::from(h22_v)),
            Arc::new(Float64Array::from(h12_v)),
            Arc::new(Float64Array::from(rg_v)),
            Arc::new(Float64Array::from(rg_lower_v)),
            Arc::new(Float64Array::from(rg_upper_v)),
            Arc::new(Float64Array::from(p_h1_v)),
            Arc::new(Float64Array::from(p_h2_v)),
            Arc::new(Float64Array::from(p_h12_v)),
            Arc::new(Float64Array::from(int_h11_v)),
            Arc::new(Float64Array::from(int_h22_v)),
            Arc::new(Float64Array::from(int_h12_v)),
            Arc::new(Int64Array::from(n_retained_v)),
            Arc::new(BooleanArray::from(converged_v)),
            Arc::new(StringArray::from(error_v)),
        ],
    )
    .map_err(|e| DagError::NodeError {
        node_type: HDL_L_SCAN_KIND.into(),
        msg: format!("arrow: {e}"),
    })
}
