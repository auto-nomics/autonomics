//! GenomicSEM DAG nodes.
//!
//! Provides structural equation modeling of GWAS summary statistics:
//!
//! - **`gsem_munge`** — QC and harmonize raw GWAS summary statistics into
//!   the canonical (rsid, z, n, a1, a2) format expected by downstream
//!   GenomicSEM nodes.
//! - **`gsem_usermodel`** — Fit a user-specified SEM to an LDSC-derived
//!   genetic covariance matrix.
//! - **`gsem_commonfactor`** — Fit a one-factor model.
//! - **`gsem_rgmodel`** — Compute model-implied genetic correlation matrix.
//!
//! All nodes accept an upstream `DataFrame` representing the S/V covariance
//! structure (from LDSC), and produce a results `DataFrame` with parameter
//! estimates, standard errors, and model fit statistics.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

use genomic_sem;

// =====================================================================
// Shared helpers
// =====================================================================

/// Read a Covstruc from a "vech" formatted DataFrame.
fn read_covstruc_from_batch(
    batch: &RecordBatch,
    n_traits: usize,
) -> Result<genomic_sem::utils::Covstruc, DagError> {
    let z = n_traits * (n_traits + 1) / 2;

    let mut s_vec = Vec::with_capacity(z);
    for i in 0..z {
        let col_name = format!("s_{i}");
        let arr = batch.column_by_name(&col_name)
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("missing column '{col_name}'"),
            })?
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("column '{col_name}' is not Float64"),
            })?;
        s_vec.push(arr.value(0));
    }
    let s = genomic_sem::linalg::vech_inv(&s_vec);

    let mut v_vec = Vec::with_capacity(z * z);
    for i in 0..(z * z) {
        let col_name = format!("v_{i}");
        let arr = batch.column_by_name(&col_name)
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("missing column '{col_name}'"),
            })?
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("column '{col_name}' is not Float64"),
            })?;
        v_vec.push(arr.value(0));
    }
    let v = Mat::from_fn(z, z, |i, j| v_vec[i * z + j]);

    let m = batch.column_by_name("m")
        .and_then(|a| a.as_any().downcast_ref::<Float64Array>())
        .map(|a| a.value(0))
        .unwrap_or(100_000.0);

    Ok(genomic_sem::utils::Covstruc {
        v,
        s,
        i_mat: Mat::<f64>::identity(n_traits, n_traits),
        n: Mat::zeros(1, z),
        m,
        v_stand: None,
        s_stand: None,
    })
}

/// Output schema for SEM results.
fn results_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("lhs", DataType::Utf8, false),
        Field::new("op", DataType::Utf8, false),
        Field::new("rhs", DataType::Utf8, false),
        Field::new("est", DataType::Float64, false),
        Field::new("se", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
        Field::new("std_all", DataType::Float64, true),
        Field::new("chisq", DataType::Float64, false),
        Field::new("df", DataType::Float64, false),
    ]))
}

/// Build a results batch from SEM output.
fn build_results_batch(
    results: &[genomic_sem::usermodel::ParamResult],
    modelfit: &genomic_sem::usermodel::ModelFit,
) -> Result<RecordBatch, DagError> {
    let n = results.len();

    let lhs: Vec<&str> = results.iter().map(|r| r.lhs.as_str()).collect();
    let op: Vec<&str> = results.iter().map(|r| r.op.as_str()).collect();
    let rhs: Vec<&str> = results.iter().map(|r| r.rhs.as_str()).collect();
    let est: Vec<f64> = results.iter().map(|r| r.unstand_est).collect();
    let se: Vec<f64> = results.iter().map(|r| r.unstand_se).collect();
    let pval: Vec<f64> = results.iter().map(|r| r.p_value).collect();
    let std_all: Vec<f64> = results.iter().map(|r| r.std_all).collect();
    let chisq: Vec<f64> = vec![modelfit.chisq; n];
    let df_vals: Vec<f64> = vec![modelfit.df as f64; n];

    let batch = RecordBatch::try_new(
        results_schema(),
        vec![
            Arc::new(StringArray::from(lhs)),
            Arc::new(StringArray::from(op)),
            Arc::new(StringArray::from(rhs)),
            Arc::new(Float64Array::from(est)),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(pval)),
            Arc::new(Float64Array::from(std_all)),
            Arc::new(Float64Array::from(chisq)),
            Arc::new(Float64Array::from(df_vals)),
        ],
    ).map_err(|e| DagError::NodeError {
        node_type: "gsem".into(),
        msg: format!("arrow error: {e}"),
    })?;

    Ok(batch)
}

// =====================================================================
// munge node
// =====================================================================

const GSEM_MUNGE_NODE_KIND: &str = "gsem_munge";

/// Output schema for munged sumstats: rsid, z, n, a1, a2.
fn munge_output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("rsid", DataType::Utf8, false),
        Field::new("z", DataType::Float64, false),
        Field::new("n", DataType::Float64, false),
        Field::new("a1", DataType::Utf8, true),
        Field::new("a2", DataType::Utf8, true),
    ]))
}

/// Config for `gsem_munge` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemMungeConfig {
    /// Optional fixed sample size. When provided, overrides the N column in
    /// the sumstats for all SNPs (useful when the GWAS file lacks per-SNP N).
    #[serde(default)]
    pub n: Option<f64>,
    /// INFO score filter threshold. SNPs with INFO below this are dropped.
    /// Ignored when the sumstats have no INFO column. Default 0.9.
    #[serde(default = "default_info_filter")]
    pub info_filter: f64,
    /// MAF filter threshold. SNPs with MAF below this are dropped.
    /// Ignored when the sumstats have no MAF column. Default 0.01.
    #[serde(default = "default_maf_filter")]
    pub maf_filter: f64,
    /// Trait name for this munge run. Stored as metadata, used by downstream
    /// multivariate LDSC to label matrix rows/columns.
    #[serde(default)]
    pub trait_name: Option<String>,
}

fn default_info_filter() -> f64 {
    0.9
}

fn default_maf_filter() -> f64 {
    0.01
}

#[derive(Clone)]
pub struct GsemMungeNode {
    meta: NodePorts,
    config: GsemMungeConfig,
}

pub struct GsemMungeNodeFactory;

impl NodeFactory for GsemMungeNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_MUNGE_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "QC and harmonize raw GWAS summary statistics (GenomicSEM munge)."
    }

    fn doc(&self) -> &'static str {
        "Takes raw GWAS summary statistics with flexible column naming (OR/BETA/Z, \
        P/PVAL, SNP/RSID, A1/A2, N, INFO, MAF) and produces a clean DataFrame with \
        rsid, z, n, a1, a2. Computes Z-scores from effect + p when Z is missing. \
        Applies INFO and MAF QC filters. This is the first step in the GenomicSEM \
        pipeline — output feeds into gsem_ldsc (multivariate LD Score regression)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemMungeConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            // Input port accepts any schema — column resolution is done at
            // runtime via genomic_sem::munge::map_column_names.
            .add_input_port(None)
            .add_output_port(Some(munge_output_schema()))
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemMungeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemMungeNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(Some(munge_output_schema())),
            config,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<GsemMungeConfig>(spec, GSEM_MUNGE_NODE_KIND)?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let tmp = ctx.fresh_var("munged_file");
        let n_flag = cfg
            .n
            .map(|v| format!(" --N {v}"))
            .unwrap_or_default();
        let trait_flag = cfg
            .trait_name
            .as_deref()
            .map(|t| format!(" --trait-name \"{t}\""))
            .unwrap_or_default();
        let code = vec![
            format!("# Munge GWAS summary statistics for GenomicSEM"),
            format!("{tmp} <- tempfile(fileext = \".sumstats.gz\")"),
            format!(
                "{tmp} <- system2(\"munge_sumstats.py\", c(\"--out\", {tmp}, \
                 \"--info\", \"{}\", \"--maf\", \"{}\"{n_flag}{trait_flag}), \
                 stdout = TRUE, stderr = TRUE)",
                cfg.info_filter, cfg.maf_filter,
            ),
            format!("{out} <- data.table::fread({tmp})"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
    }
}

#[async_trait]
impl DagNode for GsemMungeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_MUNGE_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_MUNGE_NODE_KIND.into(),
            msg: "missing input DataFrame".into(),
        })?;

        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;

        if batches.is_empty() {
            return Err(DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let munged = munge_sumstats(&batches, &self.config)?;

        let batch = RecordBatch::try_new(
            munge_output_schema(),
            vec![
                Arc::new(StringArray::from(munged.rsid)),
                Arc::new(Float64Array::from(munged.z)),
                Arc::new(Float64Array::from(munged.n)),
                Arc::new(StringArray::from(munged.a1)),
                Arc::new(StringArray::from(munged.a2)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: GSEM_MUNGE_NODE_KIND.into(),
            msg: format!("arrow error: {e}"),
        })?;

        let df = node_ctx.session().read_batch(batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Result of munging: parallel arrays of rsid, Z-score, N, A1, A2.
#[derive(Debug)]
struct MungedArrays {
    rsid: Vec<Option<String>>,
    z: Vec<Option<f64>>,
    n: Vec<Option<f64>>,
    a1: Vec<Option<String>>,
    a2: Vec<Option<String>>,
}

/// Munge raw GWAS sumstats batches into (rsid, z, n, a1, a2).
///
/// Column resolution uses [`genomic_sem::munge::map_column_names`] so the
/// upstream DataFrame can use any of the recognized aliases (BETA/OR/Z,
/// P/PVAL, SNP/RSID, etc.). Z-scores are computed from effect + p when a
/// raw Z column is absent. INFO and MAF QC filters are applied when those
/// columns are present.
fn munge_sumstats(
    batches: &[RecordBatch],
    cfg: &GsemMungeConfig,
) -> Result<MungedArrays, DagError> {
    // ── 1. Resolve column names from the first batch ──
    let first = &batches[0];
    let header: Vec<String> = first.schema().fields().iter().map(|f| f.name().clone()).collect();
    let mapping = genomic_sem::munge::map_column_names(&header);

    // mapping: original column name → canonical name.
    // We need the reverse: canonical → original (actual column name in the batch).
    let resolve = |canonical: &str| -> Option<&str> {
        mapping
            .iter()
            .find(|(_, v)| v.as_str() == canonical)
            .map(|(k, _)| k.as_str())
    };

    let snp_col = resolve("SNP").ok_or_else(|| DagError::NodeError {
        node_type: GSEM_MUNGE_NODE_KIND.into(),
        msg: "no SNP/RSID column found in sumstats".into(),
    })?;

    // Determine whether we have a direct Z column or must compute from effect+P.
    let z_col = resolve("Z").map(|s| s.to_string());
    let effect_col = resolve("effect").map(|s| s.to_string());
    let p_col = resolve("P").map(|s| s.to_string());
    let n_col = resolve("N").map(|s| s.to_string());
    let info_col = resolve("INFO").map(|s| s.to_string());
    let maf_col = resolve("MAF").map(|s| s.to_string());
    let a1_col = resolve("A1").map(|s| s.to_string());
    let a2_col = resolve("A2").map(|s| s.to_string());

    if z_col.is_none() && (effect_col.is_none() || p_col.is_none()) {
        return Err(DagError::NodeError {
            node_type: GSEM_MUNGE_NODE_KIND.into(),
            msg: "sumstats must have either a Z column, or both effect (BETA/OR) \
                  and P columns"
                .into(),
        });
    }

    // ── 2. Iterate over all batches, extracting and QC-filtering rows ──
    let mut rsid = Vec::new();
    let mut z = Vec::new();
    let mut n = Vec::new();
    let mut a1_out = Vec::new();
    let mut a2_out = Vec::new();

    for batch in batches {
        let schema = batch.schema();
        let num_rows = batch.num_rows();

        // Helper: find column index by resolved canonical name.
        let find_col = |name: &str| -> Option<usize> {
            schema.index_of(name).ok()
        };

        // SNP column is mandatory.
        let snp_idx = find_col(snp_col).ok_or_else(|| DagError::NodeError {
            node_type: GSEM_MUNGE_NODE_KIND.into(),
            msg: format!("SNP column '{snp_col}' not found in batch"),
        })?;

        let snp_vals = dag_core::node::string_opt_values(batch.column(snp_idx).as_ref())
            .ok_or_else(|| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: "SNP column is not a string type".into(),
            })?;

        // Z column: either direct or computed from effect + P.
        let (z_vals, is_or): (Vec<f64>, bool) = if let Some(zc) = z_col.as_deref() {
            let zi = find_col(zc).ok_or_else(|| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("Z column '{zc}' not found"),
            })?;
            let z_arr = batch.column(zi)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DagError::NodeError {
                    node_type: GSEM_MUNGE_NODE_KIND.into(),
                    msg: "Z column is not Float64".into(),
                })?;
            ((0..num_rows).map(|i| z_arr.value(i)).collect(), false)
        } else {
            // Compute Z from effect + P.
            let ec = effect_col.as_deref().unwrap();
            let pc = p_col.as_deref().unwrap();
            let ei = find_col(ec).ok_or_else(|| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("effect column '{ec}' not found"),
            })?;
            let pi = find_col(pc).ok_or_else(|| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("P column '{pc}' not found"),
            })?;
            let eff_arr = batch.column(ei)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DagError::NodeError {
                    node_type: GSEM_MUNGE_NODE_KIND.into(),
                    msg: "effect column is not Float64".into(),
                })?;
            let p_arr = batch.column(pi)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DagError::NodeError {
                    node_type: GSEM_MUNGE_NODE_KIND.into(),
                    msg: "P column is not Float64".into(),
                })?;
            let effects: Vec<f64> = (0..num_rows).map(|i| eff_arr.value(i)).collect();
            let or = genomic_sem::munge::is_odds_ratio(&effects);
            let zs: Vec<f64> = (0..num_rows)
                .map(|i| {
                    let eff = genomic_sem::munge::log_if_or(effects[i], or);
                    let p = p_arr.value(i);
                    if p > 0.0 && p <= 1.0 && eff.is_finite() {
                        genomic_sem::munge::z_from_p(eff, p)
                    } else {
                        f64::NAN
                    }
                })
                .collect();
            (zs, or)
        };
        let _ = is_or; // currently unused beyond z computation

        // N column: use config override if provided, else read from batch.
        let n_vals: Vec<f64> = if let Some(fixed_n) = cfg.n {
            vec![fixed_n; num_rows]
        } else if let Some(nc) = n_col.as_deref() {
            let ni = find_col(nc).ok_or_else(|| DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: format!("N column '{nc}' not found"),
            })?;
            let n_arr = batch.column(ni)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DagError::NodeError {
                    node_type: GSEM_MUNGE_NODE_KIND.into(),
                    msg: "N column is not Float64".into(),
                })?;
            (0..num_rows).map(|i| n_arr.value(i)).collect()
        } else {
            return Err(DagError::NodeError {
                node_type: GSEM_MUNGE_NODE_KIND.into(),
                msg: "no N column found and no fixed N provided in config".into(),
            });
        };

        // A1/A2 (optional — present in most GWAS files).
        let a1_vals: Vec<Option<String>> = if let Some(c) = a1_col.as_deref() {
            let idx = find_col(c).unwrap_or(usize::MAX);
            if idx != usize::MAX {
                dag_core::node::string_opt_values(batch.column(idx).as_ref())
                    .unwrap_or_else(|| vec![None; num_rows])
            } else {
                vec![None; num_rows]
            }
        } else {
            vec![None; num_rows]
        };
        let a2_vals: Vec<Option<String>> = if let Some(c) = a2_col.as_deref() {
            let idx = find_col(c).unwrap_or(usize::MAX);
            if idx != usize::MAX {
                dag_core::node::string_opt_values(batch.column(idx).as_ref())
                    .unwrap_or_else(|| vec![None; num_rows])
            } else {
                vec![None; num_rows]
            }
        } else {
            vec![None; num_rows]
        };

        // INFO filter (optional).
        let info_pass: Vec<bool> = if let Some(ic) = info_col.as_deref() {
            let idx = find_col(ic).unwrap_or(usize::MAX);
            if idx != usize::MAX {
                let arr = batch.column(idx)
                    .as_any()
                    .downcast_ref::<Float64Array>();
                (0..num_rows)
                    .map(|i| {
                        arr.and_then(|a| Some(a.value(i)))
                            .map(|v| v >= cfg.info_filter)
                            .unwrap_or(false)
                    })
                    .collect()
            } else {
                vec![true; num_rows]
            }
        } else {
            vec![true; num_rows]
        };

        // MAF filter (optional — symmetric: filter on min(maf, 1-maf)).
        let maf_pass: Vec<bool> = if let Some(mc) = maf_col.as_deref() {
            let idx = find_col(mc).unwrap_or(usize::MAX);
            if idx != usize::MAX {
                let arr = batch.column(idx)
                    .as_any()
                    .downcast_ref::<Float64Array>();
                (0..num_rows)
                    .map(|i| {
                        arr.and_then(|a| Some(a.value(i)))
                            .map(|v| {
                                let m = v.min(1.0 - v);
                                m >= cfg.maf_filter
                            })
                            .unwrap_or(false)
                    })
                    .collect()
            } else {
                vec![true; num_rows]
            }
        } else {
            vec![true; num_rows]
        };

        // ── 3. Filter and push ──
        for i in 0..num_rows {
            let snp = &snp_vals[i];
            let zv = z_vals[i];
            let nv = n_vals[i];

            // Drop rows with missing rsid, NaN z, or non-positive N.
            if snp.is_none() || zv.is_nan() || nv <= 0.0 {
                continue;
            }
            // Apply INFO and MAF filters.
            if !info_pass[i] || !maf_pass[i] {
                continue;
            }

            rsid.push(snp.clone());
            z.push(Some(zv));
            n.push(Some(nv));
            a1_out.push(a1_vals[i].clone());
            a2_out.push(a2_vals[i].clone());
        }
    }

    if rsid.is_empty() {
        return Err(DagError::NodeError {
            node_type: GSEM_MUNGE_NODE_KIND.into(),
            msg: "no SNPs survived QC filtering".into(),
        });
    }

    Ok(MungedArrays {
        rsid,
        z,
        n,
        a1: a1_out,
        a2: a2_out,
    })
}

// =====================================================================
// ldsc node — multivariate LD Score regression
// =====================================================================

const GSEM_LDSC_NODE_KIND: &str = "gsem_ldsc";

/// Config for `gsem_ldsc` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemLdscConfig {
    /// Number of traits. Must match the number of unique `trait` values in the
    /// input DataFrame.
    pub n_traits: usize,
    /// Sample prevalence for each trait (for liability scaling of binary
    /// traits). Use `None` for continuous traits.
    #[serde(default)]
    pub sample_prev: Vec<Option<f64>>,
    /// Population prevalence for each trait (for liability scaling).
    #[serde(default)]
    pub population_prev: Vec<Option<f64>>,
    /// Number of block-jackknife blocks. Default 200.
    #[serde(default = "default_n_blocks_ldsc")]
    pub n_blocks: usize,
    /// Whether to compute standardized (genetic correlation) output.
    #[serde(default)]
    pub stand: bool,
}

fn default_n_blocks_ldsc() -> usize {
    200
}

/// Build the output schema for the Covstruc DataFrame.
///
/// Columns: `s_0..s_{z-1}` (vech of S), `v_0..v_{z*z-1}` (row-major V), `m`.
/// where `z = n_traits * (n_traits + 1) / 2`.
fn ldsc_output_schema(n_traits: usize) -> SchemaRef {
    let z = n_traits * (n_traits + 1) / 2;
    let mut fields = Vec::with_capacity(z + z * z + 1);
    for i in 0..z {
        fields.push(Field::new(&format!("s_{i}"), DataType::Float64, false));
    }
    for i in 0..(z * z) {
        fields.push(Field::new(&format!("v_{i}"), DataType::Float64, false));
    }
    fields.push(Field::new("m", DataType::Float64, false));
    Arc::new(Schema::new(fields))
}

/// Build the single-row output batch from a `Covstruc`.
fn build_covstruc_batch(
    cov: &genomic_sem::utils::Covstruc,
    n_traits: usize,
) -> Result<RecordBatch, DagError> {
    let schema = ldsc_output_schema(n_traits);
    let z = n_traits * (n_traits + 1) / 2;

    // S → vech
    let s_vec = genomic_sem::linalg::vech(&cov.s);
    // V → row-major flatten
    let v_flat: Vec<f64> = (0..z)
        .flat_map(|i| (0..z).map(move |j| cov.v[(i, j)]))
        .collect();

    let mut columns: Vec<Arc<dyn arrow_array::Array>> = Vec::with_capacity(z + z * z + 1);
    // s_i columns (single value each)
    for v in &s_vec {
        columns.push(Arc::new(Float64Array::from(vec![*v])));
    }
    for v in &v_flat {
        columns.push(Arc::new(Float64Array::from(vec![*v])));
    }
    columns.push(Arc::new(Float64Array::from(vec![cov.m])));

    RecordBatch::try_new(schema, columns).map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("arrow error: {e}"),
    })
}

#[derive(Clone)]
pub struct GsemLdscNode {
    meta: NodePorts,
    config: GsemLdscConfig,
}

pub struct GsemLdscNodeFactory;

impl NodeFactory for GsemLdscNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_LDSC_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Multivariate LD Score regression → genetic covariance matrix (GenomicSEM ldsc)."
    }

    fn doc(&self) -> &'static str {
        "Takes munged GWAS sumstats for N traits (long-format DataFrame with rsid, z, n, trait), \
        joins with the univariate LD-score panel from Iceberg, runs multivariate LD Score \
        regression with block jackknife to estimate the genetic covariance matrix S and its \
        sampling covariance V. Outputs a single-row DataFrame (s_0..s_z, v_0..v_zz, m) \
        consumable by gsem_usermodel / gsem_commonfactor / gsem_rgmodel."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemLdscConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // long-format munged sumstats
            .add_output_port(Some(ldsc_output_schema(2))) // placeholder; real schema depends on n_traits
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemLdscConfig = serde_json::from_value(spec)?;
        let meta = NodePorts::new()
            .add_input_port(None)
            .add_output_port(Some(ldsc_output_schema(config.n_traits)));
        Ok(Box::new(GsemLdscNode { meta, config }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemLdscNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_LDSC_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: "missing input DataFrame".into(),
        })?;

        let session = node_ctx.session();

        // ── 1. Resolve LD-score panel from resource catalog ──
        let ld_ref = LdScoreRefCompat::resolve(&node_ctx.resources);

        // ── 2. Register the input sumstats as a temp table ──
        session
            .register_table("gsem_sumstats", input.data.clone().into_view())
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: format!("register_table failed: {e}"),
            })?;

        // ── 3. Discover trait names from the data ──
        let trait_names = discover_traits(&session).await?;

        let k = self.config.n_traits;
        if trait_names.len() != k {
            return Err(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: format!(
                    "expected {k} traits but found {} unique trait values: {:?}",
                    trait_names.len(),
                    trait_names
                ),
            });
        }

        // ── 4. Pivot sumstats to wide format, join with LD panel ──
        let joined_df = build_wide_join(&session, &trait_names, &ld_ref).await?;

        // ── 5. Extract arrays ──
        let batches = joined_df.collect().await.map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("collect failed: {e}"),
        })?;

        let extracted = extract_arrays(&batches, k, &trait_names)?;

        // ── 6. Read M_5_50 ──
        let m_sql = format!(r#"SELECT "m_5_50" FROM {}"#, ld_ref.m_sql);
        let m_df = session.sql(&m_sql).await.map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("M table query failed: {e}"),
        })?;
        let m_batches = m_df.collect().await.map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("M table collect failed: {e}"),
        })?;
        let m_value = {
            let mut val: Option<f64> = None;
            'outer: for b in &m_batches {
                if let Some(a) = b.column(0).as_any().downcast_ref::<Float64Array>() {
                    for i in 0..b.num_rows() {
                        val = Some(a.value(i));
                        break 'outer;
                    }
                }
            }
            val.ok_or(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "M table is empty".into(),
            })?
        };

        // ── 7. Run multivariate LDSC regression ──
        let covstruc = run_multivariate_ldsc(
            &extracted,
            k,
            m_value,
            self.config.n_blocks,
            &self.config.sample_prev,
            &self.config.population_prev,
            self.config.stand,
            &trait_names,
        )?;

        // ── 8. Build output ──
        let batch = build_covstruc_batch(&covstruc, k)?;
        let df = session.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("read_batch: {e}"),
        })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Compatibility wrapper — resolves LD-score panel from resource catalog.
struct LdScoreRefCompat {
    sql: String,
    m_sql: String,
}

impl LdScoreRefCompat {
    fn resolve(catalog: &dag_core::resource_catalog::ResourceCatalog) -> Self {
        match catalog.resolve_iceberg("ldscore.1000g_eur") {
            Ok(ident) => {
                let m_ident = ident.with_table_suffix("_m");
                Self {
                    sql: ident.sql(),
                    m_sql: m_ident.sql(),
                }
            }
            Err(_) => Self {
                sql: r#"iceberg.ld_score."1000g_eur""#.into(),
                m_sql: r#"iceberg.ld_score."1000g_eur_m""#.into(),
            },
        }
    }
}

/// Per-SNP arrays extracted from the joined DataFrame, ready for LDSC regression.
struct LdscArrays {
    /// k vectors of Z-scores, one per trait.
    z: Vec<Vec<f64>>,
    /// k vectors of sample sizes.
    n: Vec<Vec<f64>>,
    /// LD score (L2), one per SNP.
    l2: Vec<f64>,
    /// Weight LD score, one per SNP.
    wld: Vec<f64>,
    n_snps: usize,
}

/// Discover unique trait names from the `trait` column, sorted for determinism.
async fn discover_traits(
    session: &datafusion::prelude::SessionContext,
) -> Result<Vec<String>, DagError> {
    let df = session
        .sql(r#"SELECT DISTINCT "trait" FROM gsem_sumstats ORDER BY "trait""#)
        .await
        .map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("discover traits failed: {e}"),
        })?;
    let batches = df.collect().await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("collect traits failed: {e}"),
    })?;

    let mut traits = Vec::new();
    for batch in &batches {
        let vals = dag_core::node::string_opt_values(batch.column(0).as_ref())
            .ok_or(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "trait column is not a string type".into(),
            })?;
        for v in vals {
            traits.push(v.unwrap_or_default());
        }
    }
    Ok(traits)
}

/// Build a wide-format join: pivot N traits to per-SNP columns, inner-join with
/// the LD-score panel on rsid.
async fn build_wide_join(
    session: &datafusion::prelude::SessionContext,
    trait_names: &[String],
    ld_ref: &LdScoreRefCompat,
) -> Result<datafusion::prelude::DataFrame, DagError> {
    // Build conditional aggregation to pivot long → wide.
    // Each trait gets z_{name} and n_{name} columns.
    let mut z_cases = Vec::new();
    let mut n_cases = Vec::new();
    for t in trait_names {
        z_cases.push(format!(
            r#"MAX(CASE WHEN "trait" = '{t}' THEN "z" END) AS "z_{t}""#
        ));
        n_cases.push(format!(
            r#"MAX(CASE WHEN "trait" = '{t}' THEN "n" END) AS "n_{t}""#
        ));
    }

    let pivot_sql = format!(
        r#"SELECT "rsid", {z_cols}, {n_cols}
           FROM gsem_sumstats
           GROUP BY "rsid""#,
        z_cols = z_cases.join(", "),
        n_cols = n_cases.join(", "),
    );

    let sql = format!(
        r#"SELECT p."rsid", {z_and_n}, l."ld_score" AS "l2", l."w_ld" AS "wld"
           FROM ({pivot_sql}) AS p
           INNER JOIN {ld_table} AS l
           ON p."rsid" = l."rsid"
           WHERE {not_null_filters}"#,
        z_and_n = {
            let mut cols = Vec::new();
            for t in trait_names {
                cols.push(format!(r#"p."z_{t}" AS "z_{t}""#));
                cols.push(format!(r#"p."n_{t}" AS "n_{t}""#));
            }
            cols.join(", ")
        },
        ld_table = ld_ref.sql,
        not_null_filters = {
            let mut filters = Vec::new();
            for t in trait_names {
                filters.push(format!(r#"p."z_{t}" IS NOT NULL"#));
            }
            filters.join(" AND ")
        },
    );

    session.sql(&sql).await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("wide-join SQL failed: {e}\nSQL: {sql}"),
    })
}

/// Extract per-SNP arrays from the joined batches.
fn extract_arrays(
    batches: &[RecordBatch],
    k: usize,
    trait_names: &[String],
) -> Result<LdscArrays, DagError> {
    // Determine column indices: rsid, then [z_t0, n_t0, z_t1, n_t1, ...], l2, wld.
    // This matches the SQL output column order.
    let mut z_cols = Vec::with_capacity(k);
    let mut n_cols = Vec::with_capacity(k);
    for t in trait_names {
        z_cols.push(format!("z_{t}"));
        n_cols.push(format!("n_{t}"));
    }

    let mut z = vec![Vec::new(); k];
    let mut n = vec![Vec::new(); k];
    let mut l2 = Vec::new();
    let mut wld = Vec::new();

    for batch in batches {
        let schema = batch.schema();

        // Resolve column indices.
        let z_idx: Vec<usize> = z_cols
            .iter()
            .map(|c| schema.index_of(c).unwrap_or(usize::MAX))
            .collect();
        let n_idx: Vec<usize> = n_cols
            .iter()
            .map(|c| schema.index_of(c).unwrap_or(usize::MAX))
            .collect();
        let l2_idx = schema.index_of("l2").map_err(|_| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: "missing 'l2' column".into(),
        })?;
        let wld_idx = schema.index_of("wld").map_err(|_| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: "missing 'wld' column".into(),
        })?;

        let l2_arr = batch
            .column(l2_idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "l2 column is not Float64".into(),
            })?;
        let wld_arr = batch
            .column(wld_idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "wld column is not Float64".into(),
            })?;

        for row in 0..batch.num_rows() {
            // All z columns must be non-null (inner join guarantees this).
            let mut all_valid = true;
            for (i, &zi) in z_idx.iter().enumerate() {
                let arr = batch.column(zi)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                if arr.is_null(row) {
                    all_valid = false;
                    break;
                }
                z[i].push(arr.value(row));
            }
            if !all_valid {
                continue;
            }
            for (i, &ni) in n_idx.iter().enumerate() {
                let arr = batch.column(ni)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                n[i].push(arr.value(row));
            }
            l2.push(l2_arr.value(row));
            wld.push(wld_arr.value(row));
        }
    }

    let n_snps = l2.len();
    if n_snps == 0 {
        return Err(DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: "no SNPs survived the inner join with the LD panel".into(),
        });
    }

    Ok(LdscArrays { z, n, l2, wld, n_snps })
}

/// Run the multivariate LDSC regression: for each (i,j) pair with i ≤ j,
/// compute χ² or cross-product, run block-jackknife weighted LS, and
/// assemble the S/V/I/N covariance structure.
fn run_multivariate_ldsc(
    arrays: &LdscArrays,
    k: usize,
    m: f64,
    n_blocks: usize,
    sample_prev: &[Option<f64>],
    population_prev: &[Option<f64>],
    stand: bool,
    trait_names: &[String],
) -> Result<genomic_sem::utils::Covstruc, DagError> {
    let n_snps = arrays.n_snps;
    let z_dim = k * (k + 1) / 2;
    let n_blocks_actual = n_blocks.min(n_snps);

    // Weights: wld / (n_bar * sqrt(m)) — GenomicSEM uses this as initial weights.
    // For simplicity, use uniform weights = wld_i (the weight LD score).
    // GenomicSEM's actual weight function is more complex (IRWLS), but the
    // first-pass uses hsq_weights(ld, wld, N, M, ...). For the multivariate case,
    // we use the per-pair N_bar.

    // Pre-compute chi values for each pair (i,j) with i ≤ j.
    // Pair index follows vech order: (0,0), (1,0), (1,1), (2,0), (2,1), (2,2), ...
    let mut chi_values: Vec<Vec<f64>> = Vec::with_capacity(z_dim);
    let mut n_bars: Vec<f64> = Vec::with_capacity(z_dim);

    for j in 0..k {
        for i in 0..=j {
            let chi: Vec<f64> = (0..n_snps)
                .map(|snp| {
                    if i == j {
                        arrays.z[i][snp] * arrays.z[i][snp] // χ² for h²
                    } else {
                        arrays.z[i][snp] * arrays.z[j][snp] // Z_i·Z_j for gencov
                    }
                })
                .collect();
            // N_bar for pair (i,j) = geometric mean of per-SNP N_i and N_j
            let n_bar: f64 = (0..n_snps)
                .map(|snp| (arrays.n[i][snp] * arrays.n[j][snp]).sqrt())
                .sum::<f64>()
                / n_snps as f64;
            chi_values.push(chi);
            n_bars.push(n_bar);
        }
    }

    // Run block-jackknife regression for each pair.
    let mut s_cov = Mat::zeros(k, k);
    let mut intercepts = Mat::zeros(k, k);
    let mut v_hold = Mat::zeros(n_blocks_actual, z_dim); // pseudo-values matrix

    let mut pair_idx = 0;
    for j in 0..k {
        for i in 0..=j {
            let weights: Vec<f64> = (0..n_snps).map(|s| arrays.wld[s]).collect();
            let result = genomic_sem::ldsc::block_jackknife_regression(
                &arrays.l2,
                &chi_values[pair_idx],
                &weights,
                n_blocks_actual,
                n_bars[pair_idx],
                m,
            );

            // S[i,j] = S[j,i] = reg_tot = coef * M
            s_cov[(i, j)] = result.reg_tot;
            s_cov[(j, i)] = result.reg_tot;
            // Intercepts
            intercepts[(i, j)] = result.intercept;
            intercepts[(j, i)] = result.intercept;

            // Fill pseudo-value column for this pair.
            let pv = &result.pseudo_values_col0;
            let n_fill = pv.len().min(n_blocks_actual);
            for b in 0..n_fill {
                v_hold[(b, pair_idx)] = pv[b];
            }

            pair_idx += 1;
        }
    }

    // Assemble N vector (1 × z_dim).
    let n_vec = Mat::from_fn(1, z_dim, |_, j| n_bars[j]);

    // Liability conversion.
    let liab_s = genomic_sem::ldsc::compute_liab_s(sample_prev, population_prev, k);

    // Assemble the full Covstruc via GenomicSEM's assembly function.
    let ldsc_output = genomic_sem::ldsc::assemble_output(
        s_cov,
        &v_hold,
        n_vec,
        intercepts,
        &liab_s,
        m,
        n_blocks_actual,
        trait_names.to_vec(),
        stand,
    );

    Ok(ldsc_output.to_covstruc())
}

// =====================================================================
// usermodel node
// =====================================================================

const GSEM_USERMODEL_NODE_KIND: &str = "gsem_usermodel";

/// Config for `gsem_usermodel` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemUsermodelConfig {
    /// Lavaan-style model syntax (e.g., "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1").
    pub model: String,
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
    /// Estimation method: "DWLS" or "ML".
    #[serde(default = "default_estimation")]
    pub estimation: String,
    /// Standardize latent variances.
    #[serde(default)]
    pub std_lv: bool,
}

fn default_estimation() -> String {
    "DWLS".to_string()
}

fn gsem_usermodel_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(results_schema()))
}

#[derive(Clone)]
pub struct GsemUsermodelNode {
    meta: NodePorts,
    config: GsemUsermodelConfig,
}

pub struct GsemUsermodelNodeFactory;

impl NodeFactory for GsemUsermodelNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_USERMODEL_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "User-specified SEM on genetic covariance matrix (GenomicSEM usermodel)."
    }

    fn doc(&self) -> &'static str {
        "Fits a structural equation model to the LDSC-derived genetic covariance matrix S \
        using DWLS or ML estimation with sandwich-corrected standard errors. \
        Specify the model in lavaan syntax (e.g., 'F1 =~ NA*V1 + V2 + V3')."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemUsermodelConfig)
    }

    fn ports(&self) -> NodePorts {
        gsem_usermodel_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemUsermodelConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemUsermodelNode {
            meta: gsem_usermodel_port_layout(),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemUsermodelNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_USERMODEL_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_USERMODEL_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;

        let estimation = match self.config.estimation.as_str() {
            "ML" => genomic_sem::sem::EstimationMethod::ML,
            _ => genomic_sem::sem::EstimationMethod::DWLS,
        };

        let user_config = genomic_sem::usermodel::UserModelConfig {
            estimation,
            model: self.config.model.clone(),
            std_lv: self.config.std_lv,
            ..Default::default()
        };

        let result = genomic_sem::usermodel::usermodel(&covstruc, &user_config)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// commonfactor node
// =====================================================================

const GSEM_COMMONFACTOR_NODE_KIND: &str = "gsem_commonfactor";

/// Config for `gsem_commonfactor` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemCommonfactorConfig {
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
    /// Estimation method: "DWLS" or "ML".
    #[serde(default = "default_estimation")]
    pub estimation: String,
}

#[derive(Clone)]
pub struct GsemCommonfactorNode {
    meta: NodePorts,
    config: GsemCommonfactorConfig,
}

pub struct GsemCommonfactorNodeFactory;

impl NodeFactory for GsemCommonfactorNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_COMMONFACTOR_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Common factor model on genetic covariance matrix (GenomicSEM commonfactor)."
    }

    fn doc(&self) -> &'static str {
        "Fits a single-factor confirmatory model to the LDSC-derived genetic \
        covariance matrix. Requires at least 3 traits."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemCommonfactorConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(Some(results_schema()))
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemCommonfactorConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemCommonfactorNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(Some(results_schema())),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemCommonfactorNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_COMMONFACTOR_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;

        let estimation = match self.config.estimation.as_str() {
            "ML" => genomic_sem::sem::EstimationMethod::ML,
            _ => genomic_sem::sem::EstimationMethod::DWLS,
        };

        let config = genomic_sem::commonfactor::CommonFactorConfig { estimation };
        let result = genomic_sem::commonfactor::commonfactor(&covstruc, &config)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// rgmodel node
// =====================================================================

const GSEM_RGMODEL_NODE_KIND: &str = "gsem_rgmodel";

/// Config for `gsem_rgmodel` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemRgmodelConfig {
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
}

fn rgmodel_output_schema(k: usize) -> SchemaRef {
    let z = k * (k + 1) / 2;
    let mut fields = Vec::new();
    for i in 0..z {
        fields.push(Field::new(&format!("r_{i}"), DataType::Float64, false));
    }
    for i in 0..z {
        fields.push(Field::new(&format!("v_r_{i}"), DataType::Float64, false));
    }
    Arc::new(Schema::new(fields))
}

#[derive(Clone)]
pub struct GsemRgmodelNode {
    meta: NodePorts,
    config: GsemRgmodelConfig,
}

pub struct GsemRgmodelNodeFactory;

impl NodeFactory for GsemRgmodelNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_RGMODEL_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Model-implied genetic correlation matrix (GenomicSEM rgmodel)."
    }

    fn doc(&self) -> &'static str {
        "Computes the genetic correlation matrix R = cov2cor(S) and its \
        sampling covariance V_R via the delta method."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemRgmodelConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None) // schema depends on n_traits at runtime
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemRgmodelConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemRgmodelNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(Some(rgmodel_output_schema(config.n_traits))),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemRgmodelNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_RGMODEL_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_RGMODEL_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;
        let result = genomic_sem::rgmodel::rgmodel(&covstruc, false)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let k = self.config.n_traits;
        let z = k * (k + 1) / 2;
        let r_vec = genomic_sem::linalg::vech(&result.r);

        let mut columns: Vec<Arc<dyn Array>> = Vec::new();
        for i in 0..z {
            columns.push(Arc::new(Float64Array::from(vec![r_vec[i]])));
        }
        for i in 0..z {
            columns.push(Arc::new(Float64Array::from(vec![result.v_r[(i, i)]])));
        }

        let output_batch = RecordBatch::try_new(
            rgmodel_output_schema(k),
            columns,
        ).map_err(|e| DagError::NodeError {
            node_type: GSEM_RGMODEL_NODE_KIND.into(),
            msg: format!("arrow: {e}"),
        })?;

        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
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
    use arrow_array::Float64Array;
    use std::sync::Arc as Arc2;

    /// Build a raw GWAS sumstats batch with typical column names.
    fn make_sumstats_batch() -> RecordBatch {
        let schema = Arc2::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("A1", DataType::Utf8, false),
            Field::new("A2", DataType::Utf8, false),
            Field::new("BETA", DataType::Float64, false),
            Field::new("P", DataType::Float64, false),
            Field::new("N", DataType::Float64, false),
            Field::new("INFO", DataType::Float64, false),
            Field::new("MAF", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(StringArray::from(vec!["rs1", "rs2", "rs3", "rs4"])),
                Arc2::new(StringArray::from(vec!["A", "C", "G", "T"])),
                Arc2::new(StringArray::from(vec!["G", "T", "A", "C"])),
                Arc2::new(Float64Array::from(vec![0.5, -0.3, 1.2, 0.0])),
                Arc2::new(Float64Array::from(vec![0.01, 0.5, 0.001, 1.0])),
                Arc2::new(Float64Array::from(vec![1000.0, 2000.0, 1500.0, 3000.0])),
                Arc2::new(Float64Array::from(vec![0.95, 0.8, 0.99, 0.5])),
                Arc2::new(Float64Array::from(vec![0.3, 0.45, 0.02, 0.005])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_munge_node_structure() {
        let node = GsemMungeNode::new(GsemMungeConfig {
            n: None,
            info_filter: 0.9,
            maf_filter: 0.01,
            trait_name: None,
        });
        assert_eq!(node.kind(), "gsem_munge");
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    #[test]
    fn test_munge_output_schema() {
        let schema = munge_output_schema();
        assert!(schema.field_with_name("rsid").is_ok());
        assert!(schema.field_with_name("z").is_ok());
        assert!(schema.field_with_name("n").is_ok());
        assert!(schema.field_with_name("a1").is_ok());
        assert!(schema.field_with_name("a2").is_ok());
    }

    /// INFO filter (≥0.9) removes rs2 (0.8) and rs4 (0.5).
    /// MAF filter (≥0.01) removes rs4 (0.005 — but already removed by INFO).
    /// Result: rs1, rs3 survive.
    #[test]
    fn test_munge_filters_info_and_maf() {
        let batch = make_sumstats_batch();
        let cfg = GsemMungeConfig {
            n: None,
            info_filter: 0.9,
            maf_filter: 0.01,
            trait_name: None,
        };
        let result = munge_sumstats(&[batch], &cfg).unwrap();

        assert_eq!(result.rsid.len(), 2);
        assert_eq!(result.rsid[0].as_deref(), Some("rs1"));
        assert_eq!(result.rsid[1].as_deref(), Some("rs3"));

        // Z computed from BETA + P: rs1 has BETA=0.5, P=0.01 → Z≈2.556
        let z0 = result.z[0].unwrap();
        assert!(z0 > 2.5 && z0 < 2.6);

        // N passes through from column.
        assert_eq!(result.n[0], Some(1000.0));
        assert_eq!(result.n[1], Some(1500.0));

        // A1/A2 pass through.
        assert_eq!(result.a1[0].as_deref(), Some("A"));
        assert_eq!(result.a2[0].as_deref(), Some("G"));
    }

    /// Fixed N override: all SNPs get the config N regardless of column.
    #[test]
    fn test_munge_fixed_n_override() {
        let batch = make_sumstats_batch();
        let cfg = GsemMungeConfig {
            n: Some(5000.0),
            info_filter: 0.0,
            maf_filter: 0.0,
            trait_name: None,
        };
        let result = munge_sumstats(&[batch], &cfg).unwrap();

        assert_eq!(result.rsid.len(), 4);
        assert!(result.n.iter().all(|n| *n == Some(5000.0)));
    }

    /// Direct Z column is preferred over effect+P.
    #[test]
    fn test_munge_direct_z_column() {
        let schema = Arc2::new(Schema::new(vec![
            Field::new("SNP", DataType::Utf8, false),
            Field::new("Z", DataType::Float64, false),
            Field::new("N", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(StringArray::from(vec!["rs1", "rs2"])),
                Arc2::new(Float64Array::from(vec![2.5, -1.5])),
                Arc2::new(Float64Array::from(vec![1000.0, 2000.0])),
            ],
        )
        .unwrap();

        let cfg = GsemMungeConfig {
            n: None,
            info_filter: 0.0,
            maf_filter: 0.0,
            trait_name: None,
        };
        let result = munge_sumstats(&[batch], &cfg).unwrap();
        assert_eq!(result.rsid.len(), 2);
        assert_eq!(result.z[0], Some(2.5));
        assert_eq!(result.z[1], Some(-1.5));
    }

    /// Error when neither Z nor (effect + P) is available.
    #[test]
    fn test_munge_error_missing_z_and_effect() {
        let schema = Arc2::new(Schema::new(vec![
            Field::new("SNP", DataType::Utf8, false),
            Field::new("N", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(StringArray::from(vec!["rs1"])),
                Arc2::new(Float64Array::from(vec![1000.0])),
            ],
        )
        .unwrap();

        let cfg = GsemMungeConfig {
            n: None,
            info_filter: 0.0,
            maf_filter: 0.0,
            trait_name: None,
        };
        let err = munge_sumstats(&[batch], &cfg);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("Z") || msg.contains("effect"));
    }

    impl GsemMungeNode {
        fn new(config: GsemMungeConfig) -> Self {
            Self {
                meta: NodePorts::new()
                    .add_input_port(None)
                    .add_output_port(Some(munge_output_schema())),
                config,
            }
        }
    }

    // ── gsem_ldsc tests ──────────────────────────────────────────

    #[test]
    fn test_ldsc_output_schema_2_traits() {
        // k=2 → z=3 → 3 + 9 + 1 = 13 columns
        let schema = ldsc_output_schema(2);
        assert_eq!(schema.fields().len(), 13);
        assert!(schema.field_with_name("s_0").is_ok());
        assert!(schema.field_with_name("s_2").is_ok());
        assert!(schema.field_with_name("v_0").is_ok());
        assert!(schema.field_with_name("v_8").is_ok());
        assert!(schema.field_with_name("m").is_ok());
    }

    #[test]
    fn test_ldsc_output_schema_3_traits() {
        // k=3 → z=6 → 6 + 36 + 1 = 43 columns
        let schema = ldsc_output_schema(3);
        assert_eq!(schema.fields().len(), 43);
    }

    #[test]
    fn test_build_covstruc_batch_roundtrip() {
        // Build a known Covstruc, serialize to batch, verify values.
        let k = 2;
        let cov = genomic_sem::utils::Covstruc {
            s: Mat::from_fn(2, 2, |i, j| if i == j { 0.25 } else { 0.10 }),
            v: Mat::from_fn(3, 3, |i, j| if i == j { 1e-4 } else { 0.0 }),
            i_mat: Mat::identity(2, 2),
            n: Mat::from_fn(1, 3, |_, _| 1000.0),
            m: 500_000.0,
            v_stand: None,
            s_stand: None,
        };

        let batch = build_covstruc_batch(&cov, k).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 13);

        // S vech for [[0.25, 0.10], [0.10, 0.25]] = [0.25, 0.10, 0.25]
        // (vech is column-major: (0,0), (1,0), (1,1))
        let s0 = batch
            .column_by_name("s_0")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!((s0 - 0.25).abs() < 1e-10);

        // m
        let m_val = batch
            .column_by_name("m")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert!((m_val - 500_000.0).abs() < 1e-6);
    }

    /// Synthetic multivariate LDSC: 2 traits, small N.
    /// Verifies that run_multivariate_ldsc produces a valid 2×2 S matrix
    /// with positive diagonal (h² > 0).
    #[test]
    fn test_run_multivariate_ldsc_synthetic() {
        let n_snps = 500;
        // Simulate χ² = 1 + slope·L2, where slope = h²·N/M.
        // Choose h²=0.2, N=1000, M=1000 → slope=0.2.
        let l2: Vec<f64> = (0..n_snps).map(|i| 1.0 + (i as f64) * 0.02).collect();
        let wld: Vec<f64> = vec![1.0; n_snps];

        // Z_i: sign alternates to keep mean ≈ 0, magnitude encodes χ² signal.
        let z1: Vec<f64> = (0..n_snps)
            .map(|i| {
                let chisq = 1.0 + 0.2 * l2[i]; // intercept=1, slope=0.2
                let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                sign * chisq.sqrt()
            })
            .collect();
        let z2: Vec<f64> = (0..n_snps)
            .map(|i| {
                let chisq = 1.0 + 0.15 * l2[i]; // slightly weaker h²
                let sign = if i % 3 == 0 { 1.0 } else { -1.0 };
                sign * chisq.sqrt()
            })
            .collect();
        let n1: Vec<f64> = vec![1000.0; n_snps];
        let n2: Vec<f64> = vec![1000.0; n_snps];

        let arrays = LdscArrays {
            z: vec![z1, z2],
            n: vec![n1, n2],
            l2,
            wld,
            n_snps,
        };

        let result = run_multivariate_ldsc(
            &arrays,
            2,
            1000.0, // M
            10,     // few blocks for speed
            &[None, None], // continuous traits
            &[None, None],
            false,         // no standardization
            &["t1".into(), "t2".into()],
        )
        .unwrap();

        // S should be 2×2 with positive diagonal (h² > 0).
        assert!(
            result.s[(0, 0)] > 0.0,
            "h²_1 should be positive, got {}",
            result.s[(0, 0)]
        );
        assert!(
            result.s[(1, 1)] > 0.0,
            "h²_2 should be positive, got {}",
            result.s[(1, 1)]
        );

        // V should be 3×3 (z=3 for k=2).
        assert_eq!(result.v.nrows(), 3);
        assert_eq!(result.v.ncols(), 3);
    }
}
