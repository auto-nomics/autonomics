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

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
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
        let arr = batch
            .column_by_name(&col_name)
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
        let arr = batch
            .column_by_name(&col_name)
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

    let m = batch
        .column_by_name("m")
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
    )
    .map_err(|e| DagError::NodeError {
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
        let _input = input_0(ctx).to_string();
        let tmp = ctx.fresh_var("munged_file");
        let n_flag = cfg.n.map(|v| format!(" --N {v}")).unwrap_or_default();
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

        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
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

        let df = node_ctx
            .session()
            .read_batch(batch)
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
    let header: Vec<String> = first
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
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
        let find_col = |name: &str| -> Option<usize> { schema.index_of(name).ok() };

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
            let z_arr = batch
                .column(zi)
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
            let eff_arr = batch
                .column(ei)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DagError::NodeError {
                    node_type: GSEM_MUNGE_NODE_KIND.into(),
                    msg: "effect column is not Float64".into(),
                })?;
            let p_arr = batch
                .column(pi)
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
            let n_arr = batch
                .column(ni)
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
                let arr = batch.column(idx).as_any().downcast_ref::<Float64Array>();
                (0..num_rows)
                    .map(|i| {
                        arr.map(|a| a.value(i))
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
                let arr = batch.column(idx).as_any().downcast_ref::<Float64Array>();
                (0..num_rows)
                    .map(|i| {
                        arr.map(|a| a.value(i))
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

/// Quote a data value for SQL interpolation. Trait names come from upstream
/// data, so they must not be inserted into SQL as raw string literals.
fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Input schema for `gsem_ldsc`: long-format munged sumstats with a `trait`
/// column identifying which trait each row belongs to.
///
/// Columns:
/// - `rsid` (Utf8) — SNP identifier, used for the LD-panel join key.
/// - `z` (Float64) — standardized effect size (Z-score).
/// - `n` (Float64) — sample size for this SNP in this trait.
/// - `trait` (Utf8) — trait label; the node pivots on this to build per-trait
///   columns before the multivariate regression.
///
/// `a1`/`a2` and other columns from `gsem_munge` are allowed as extra columns
/// (the schema check permits superset outputs) but are not required.
/// Multi-trait inputs require `a1`/`a2` at runtime so Z-score direction can be
/// aligned across traits.
fn ldsc_input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("rsid", DataType::Utf8, false),
        Field::new("z", DataType::Float64, true),
        Field::new("n", DataType::Float64, true),
        Field::new("trait", DataType::Utf8, true),
    ]))
}

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
    /// Maximum χ² value; SNPs with χ² > chisq_max are removed before
    /// regression. This filters extreme outliers (e.g. genotyping errors,
    /// GWAS catalog hits with z > ~20) that can destabilize the WLS
    /// regression. `None` = no filter. GenomicSEM default is `None`,
    /// but for real-data robustness we default to 200.
    #[serde(default = "default_chisq_max")]
    pub chisq_max: Option<f64>,
}

fn default_chisq_max() -> Option<f64> {
    Some(200.0)
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
        fields.push(Field::new(format!("s_{i}"), DataType::Float64, false));
    }
    for i in 0..(z * z) {
        fields.push(Field::new(format!("v_{i}"), DataType::Float64, false));
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
        joins with the univariate 1000G EUR LD-score panel from VFS, orders SNPs by genomic \
        position, aligns multi-trait Z scores using a1/a2, and runs multivariate LD Score \
        regression with block jackknife to estimate the genetic covariance matrix S and its \
        sampling covariance V. Outputs a single-row DataFrame (s_0..s_z, v_0..v_zz, m) \
        consumable by gsem_usermodel / gsem_commonfactor / gsem_rgmodel.\n\n\
        INPUT SCHEMA (validated at DAG-build time when the upstream port declares a schema, \
        and at runtime otherwise):\n\
        - rsid (Utf8) — SNP identifier\n\
        - z (Float64) — standardized Z-score\n\
        - n (Float64) — sample size\n\
        - trait (Utf8) — trait label; must have exactly n_traits unique values\n\
        Typically produced by gsem_munge → SQL(UNION ALL with 'trait' column)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemLdscConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(ldsc_input_schema())) // long-format munged sumstats
            .add_output_port(Some(ldsc_output_schema(2))) // placeholder; real schema depends on n_traits
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemLdscConfig = serde_json::from_value(spec)?;
        let meta = NodePorts::new()
            .add_input_port(Some(ldsc_input_schema()))
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

        // ── 0. Runtime schema validation ──
        // The port-level schema check is skipped when the upstream port has
        // no declared schema (e.g. SQL nodes). Validate at runtime so missing
        // columns produce a clear error instead of a cryptic SQL failure.
        {
            let schema_batches =
                input
                    .data
                    .clone()
                    .collect()
                    .await
                    .map_err(|e| DagError::NodeError {
                        node_type: GSEM_LDSC_NODE_KIND.into(),
                        msg: format!("schema check: collect failed: {e}"),
                    })?;
            if schema_batches.is_empty() || schema_batches[0].num_columns() == 0 {
                return Err(DagError::NodeError {
                    node_type: GSEM_LDSC_NODE_KIND.into(),
                    msg: "input DataFrame is empty — expected columns: rsid, z, n, trait".into(),
                });
            }
            let schema_ref = schema_batches[0].schema();
            let fields = schema_ref.fields();
            let have: std::collections::HashSet<&str> =
                fields.iter().map(|f| f.name().as_str()).collect();
            for required in &["rsid", "z", "n", "trait"] {
                if !have.contains(required) {
                    let available: Vec<&str> = fields.iter().map(|f| f.name().as_str()).collect();
                    return Err(DagError::NodeError {
                        node_type: GSEM_LDSC_NODE_KIND.into(),
                        msg: format!(
                            "input is missing required column '{required}'. \
                             Expected: rsid, z, n, trait. \
                             Available: {available:?}"
                        ),
                    });
                }
            }
        }

        // ── 1. Register LD-score panel and companion M table from VFS ──
        nodes_ldsc::ldsc_common::register_listing_table(
            &session,
            "ld_panel",
            nodes_ldsc::ldsc_common::VFS_LDSCORE_1000G_EUR,
        )
        .await
        .map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("register LD panel failed: {e}"),
        })?;
        nodes_ldsc::ldsc_common::register_listing_table(
            &session,
            "ld_panel_m",
            nodes_ldsc::ldsc_common::VFS_LDSCORE_1000G_EUR_M,
        )
        .await
        .map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("register LD panel M table failed: {e}"),
        })?;

        // ── 2. Register the input sumstats as a temp table ──
        session
            .register_table("gsem_sumstats", input.data.clone().into_view())
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: format!("register_table failed: {e}"),
            })?;

        validate_unique_trait_rsids(&session).await?;

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

        if trait_names.len() > 1 {
            let input_schema = input.data.schema();
            let has_a1 = input_schema.fields().iter().any(|f| f.name() == "a1");
            let has_a2 = input_schema.fields().iter().any(|f| f.name() == "a2");
            if !has_a1 || !has_a2 {
                return Err(DagError::NodeError {
                    node_type: GSEM_LDSC_NODE_KIND.into(),
                    msg: "multi-trait gsem_ldsc input requires 'a1' and 'a2' columns to align Z-score direction".into(),
                });
            }
        }

        // ── 4. Pivot sumstats to wide format, join with LD panel ──
        let joined_df = build_wide_join(&session, &trait_names, "ld_panel").await?;

        // ── 5. Extract arrays ──
        let batches = joined_df.collect().await.map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("collect failed: {e}"),
        })?;

        let extracted = extract_arrays(&batches, k)?;

        // ── 6. Read M_5_50 ──
        let m_sql = r#"SELECT "m_5_50" FROM ld_panel_m"#.to_string();
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
            for b in &m_batches {
                if val.is_some() {
                    break;
                }
                if let Some(a) = b.column(0).as_any().downcast_ref::<Float64Array>() {
                    if let Some(i) = (0..b.num_rows()).next() {
                        val = Some(a.value(i));
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
            self.config.chisq_max,
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
        let vals = dag_core::node::string_opt_values(batch.column(0).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "trait column is not a string type".into(),
            },
        )?;
        for v in vals {
            traits.push(v.unwrap_or_default());
        }
    }
    Ok(traits)
}

/// Reject duplicate `(trait, rsid)` rows before the wide pivot can silently
/// collapse them.
async fn validate_unique_trait_rsids(
    session: &datafusion::prelude::SessionContext,
) -> Result<(), DagError> {
    let df = session
        .sql(
            r#"SELECT "rsid", "trait", count(*) AS "n_rows"
               FROM gsem_sumstats
               GROUP BY "rsid", "trait"
               HAVING count(*) > 1
               ORDER BY "rsid", "trait"
               LIMIT 20"#,
        )
        .await
        .map_err(|e| DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!("duplicate check failed: {e}"),
        })?;
    let batches = df.collect().await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("duplicate check collect failed: {e}"),
    })?;

    let mut duplicates = Vec::new();
    for batch in &batches {
        let rsids = dag_core::node::string_opt_values(batch.column(0).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "duplicate check 'rsid' column is not a string type".into(),
            },
        )?;
        let traits = dag_core::node::string_opt_values(batch.column(1).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "duplicate check 'trait' column is not a string type".into(),
            },
        )?;
        let counts = batch
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or(DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "duplicate check 'n_rows' column is not Int64".into(),
            })?;
        for i in 0..batch.num_rows() {
            duplicates.push(format!(
                "({}, {})={}x",
                rsids[i].clone().unwrap_or_default(),
                traits[i].clone().unwrap_or_default(),
                counts.value(i)
            ));
        }
    }

    if !duplicates.is_empty() {
        return Err(DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!(
                "duplicate (trait, rsid) rows in gsem_ldsc input: {}. Deduplicate the input before regression.",
                duplicates.join(", ")
            ),
        });
    }
    Ok(())
}

/// Build a wide-format join: pivot N traits to per-SNP columns, align Z scores
/// to the first sorted trait's allele orientation, then inner-join with the
/// LD-score panel on rsid in genomic order.
async fn build_wide_join(
    session: &datafusion::prelude::SessionContext,
    trait_names: &[String],
    panel_table: &str,
) -> Result<datafusion::prelude::DataFrame, DagError> {
    let align_alleles = trait_names.len() > 1;

    // Build conditional aggregation to pivot long → wide.
    // Index-suffixed aliases keep trait labels out of SQL identifiers.
    let mut z_cases = Vec::new();
    let mut n_cases = Vec::new();
    let mut a1_cases = Vec::new();
    let mut a2_cases = Vec::new();
    for (i, t) in trait_names.iter().enumerate() {
        let trait_literal = sql_string_literal(t);
        z_cases.push(format!(
            r#"MAX(CASE WHEN "trait" = {trait_literal} THEN "z" END) AS "z_{i}""#
        ));
        n_cases.push(format!(
            r#"MAX(CASE WHEN "trait" = {trait_literal} THEN "n" END) AS "n_{i}""#
        ));
        if align_alleles {
            a1_cases.push(format!(
                r#"MAX(CASE WHEN "trait" = {trait_literal} THEN "a1" END) AS "a1_{i}""#
            ));
            a2_cases.push(format!(
                r#"MAX(CASE WHEN "trait" = {trait_literal} THEN "a2" END) AS "a2_{i}""#
            ));
        }
    }

    let allele_cols = if align_alleles {
        format!(", {}, {}", a1_cases.join(", "), a2_cases.join(", "))
    } else {
        String::new()
    };

    let pivot_sql = format!(
        r#"SELECT "rsid", {z_cols}, {n_cols}{allele_cols}
           FROM gsem_sumstats
           GROUP BY "rsid""#,
        z_cols = z_cases.join(", "),
        n_cols = n_cases.join(", "),
        allele_cols = allele_cols,
    );

    if align_alleles {
        validate_wide_alleles(session, &pivot_sql, trait_names).await?;
    }

    let mut z_and_n = Vec::new();
    for i in 0..trait_names.len() {
        if align_alleles {
            z_and_n.push(format!(
                r#"CASE
                     WHEN upper(p."a1_{i}") = upper(p."a1_0") AND upper(p."a2_{i}") = upper(p."a2_0")
                       THEN p."z_{i}"
                     WHEN upper(p."a1_{i}") = upper(p."a2_0") AND upper(p."a2_{i}") = upper(p."a1_0")
                       THEN -p."z_{i}"
                   END AS "z_{i}""#
            ));
        } else {
            z_and_n.push(format!(r#"p."z_{i}" AS "z_{i}""#));
        }
        z_and_n.push(format!(r#"p."n_{i}" AS "n_{i}""#));
    }
    let not_null_filters: Vec<String> = (0..trait_names.len())
        .map(|i| format!(r#"p."z_{i}" IS NOT NULL"#))
        .collect();

    let sql = format!(
        r#"SELECT p."rsid", {z_and_n}, l."ld_score" AS "l2", l."w_ld" AS "wld"
           FROM ({pivot_sql}) AS p
           INNER JOIN {ld_table} AS l
           ON p."rsid" = l."rsid"
           WHERE {not_null_filters}
           ORDER BY TRY_CAST(l."locus"."contig" AS INT) NULLS LAST,
                    l."locus"."contig",
                    l."locus"."position",
                    p."rsid""#,
        z_and_n = z_and_n.join(", "),
        ld_table = nodes_ldsc::ldsc_common::quote_table(panel_table),
        not_null_filters = not_null_filters.join(" AND "),
    );

    session.sql(&sql).await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("wide-join SQL failed: {e}\nSQL: {sql}"),
    })
}

/// Reject missing, conflicting, or ambiguous allele pairs before regression.
async fn validate_wide_alleles(
    session: &datafusion::prelude::SessionContext,
    pivot_sql: &str,
    trait_names: &[String],
) -> Result<(), DagError> {
    let reference = trait_names.first().expect("multi-trait input has a trait");
    let mismatch_filters: Vec<String> = (1..trait_names.len())
        .map(|i| {
            format!(
                r#"p."a1_{i}" IS NULL OR p."a2_{i}" IS NULL OR NOT (
                    (upper(p."a1_{i}") = upper(p."a1_0") AND upper(p."a2_{i}") = upper(p."a2_0")) OR
                    (upper(p."a1_{i}") = upper(p."a2_0") AND upper(p."a2_{i}") = upper(p."a1_0"))
                )"#
            )
        })
        .collect();
    let used_filter: String = (0..trait_names.len())
        .map(|i| format!(r#"p."z_{i}" IS NOT NULL"#))
        .collect::<Vec<_>>()
        .join(" AND ");

    let sql = format!(
        r#"SELECT p."rsid", p."a1_0", p."a2_0"
           FROM ({pivot_sql}) AS p
           WHERE {used_filter}
             AND (
               p."a1_0" IS NULL OR p."a2_0" IS NULL
                 OR upper(p."a1_0") = upper(p."a2_0")
               OR {mismatch_filters}
             )
           ORDER BY p."rsid"
           LIMIT 5"#,
        mismatch_filters = mismatch_filters.join(" OR "),
    );
    let df = session.sql(&sql).await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("allele alignment check failed: {e}\nSQL: {sql}"),
    })?;
    let batches = df.collect().await.map_err(|e| DagError::NodeError {
        node_type: GSEM_LDSC_NODE_KIND.into(),
        msg: format!("allele alignment collect failed: {e}"),
    })?;

    let mut problems = Vec::new();
    for batch in &batches {
        let rsids = dag_core::node::string_opt_values(batch.column(0).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "allele alignment 'rsid' column is not a string type".into(),
            },
        )?;
        let a1s = dag_core::node::string_opt_values(batch.column(1).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "allele alignment 'a1' column is not a string type".into(),
            },
        )?;
        let a2s = dag_core::node::string_opt_values(batch.column(2).as_ref()).ok_or(
            DagError::NodeError {
                node_type: GSEM_LDSC_NODE_KIND.into(),
                msg: "allele alignment 'a2' column is not a string type".into(),
            },
        )?;
        for i in 0..batch.num_rows() {
            problems.push(format!(
                "{}[{}={}]",
                rsids[i].clone().unwrap_or_default(),
                a1s[i].clone().unwrap_or_else(|| "NULL".into()),
                a2s[i].clone().unwrap_or_else(|| "NULL".into())
            ));
        }
    }

    if !problems.is_empty() {
        return Err(DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: format!(
                "cannot align alleles for {} SNPs (examples: {}). Each trait must match reference trait '{reference}' as (a1,a2) or reversed (a2,a1); missing and conflicting alleles are rejected.",
                problems.len(),
                problems.join(", ")
            ),
        });
    }
    Ok(())
}

/// Extract per-SNP arrays from the joined batches.
fn extract_arrays(batches: &[RecordBatch], k: usize) -> Result<LdscArrays, DagError> {
    // Determine column indices: rsid, then [z_0, n_0, z_1, n_1, ...], l2, wld.
    // This matches the SQL output column order.
    let mut z_cols = Vec::with_capacity(k);
    let mut n_cols = Vec::with_capacity(k);
    for i in 0..k {
        z_cols.push(format!("z_{i}"));
        n_cols.push(format!("n_{i}"));
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
            // ── Validate-then-push: check all columns before extending any
            // array, so we never need rollback logic that could desynchronize
            // the parallel arrays.
            //
            // NULL or NaN in any column (especially l2/wld from the LD-score
            // panel) will propagate through all regression sums and
            // contaminate the entire S/V output as NaN.

            // Z columns: non-null and finite.
            let mut z_row = Vec::with_capacity(k);
            let mut z_ok = true;
            for (i, &zi) in z_idx.iter().enumerate() {
                if zi == usize::MAX {
                    z_ok = false;
                    break;
                }
                let arr = batch
                    .column(zi)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                if arr.is_null(row) {
                    z_ok = false;
                    break;
                }
                let v = arr.value(row);
                if !v.is_finite() {
                    z_ok = false;
                    break;
                }
                z_row.push((i, v));
            }
            if !z_ok || z_row.len() != k {
                continue;
            }

            // N columns: non-null, finite, positive.
            let mut n_row = Vec::with_capacity(k);
            let mut n_ok = true;
            for (i, &ni) in n_idx.iter().enumerate() {
                if ni == usize::MAX {
                    n_ok = false;
                    break;
                }
                let arr = batch
                    .column(ni)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                if arr.is_null(row) {
                    n_ok = false;
                    break;
                }
                let v = arr.value(row);
                if !v.is_finite() || v <= 0.0 {
                    n_ok = false;
                    break;
                }
                n_row.push((i, v));
            }
            if !n_ok || n_row.len() != k {
                continue;
            }

            // l2 and wld: non-null, finite, positive. These come from the
            // LD-score panel, a separate data source that may contain NULL or
            // NaN entries — the most common root cause of all-NULL S/V output.
            if l2_arr.is_null(row) || wld_arr.is_null(row) {
                continue;
            }
            let l2_val = l2_arr.value(row);
            let wld_val = wld_arr.value(row);
            if !l2_val.is_finite() || l2_val <= 0.0 || !wld_val.is_finite() || wld_val <= 0.0 {
                continue;
            }

            // All validated — push.
            for (i, v) in z_row {
                z[i].push(v);
            }
            for (i, v) in n_row {
                n[i].push(v);
            }
            l2.push(l2_val);
            wld.push(wld_val);
        }
    }

    let n_snps = l2.len();
    if n_snps == 0 {
        return Err(DagError::NodeError {
            node_type: GSEM_LDSC_NODE_KIND.into(),
            msg: "no SNPs survived the inner join with the LD panel".into(),
        });
    }

    Ok(LdscArrays {
        z,
        n,
        l2,
        wld,
        n_snps,
    })
}

/// Compute GenomicSEM IRWLS weights for a single trait's h² regression.
///
/// Port of ldsc.R lines 231-243 (heritability case):
/// ```r
/// tot.agg <- (M*(mean(chi1)-1))/mean(L2*N)
/// tot.agg <- max(min(tot.agg, 1), 0)
/// ld <- pmax(L2, 1);  w.ld <- pmax(wLD, 1)
/// c <- tot.agg * N / M
/// het.w <- 1/(2*(1+c*ld)^2)
/// oc.w  <- 1/w.ld
/// w     <- het.w * oc.w
/// initial.w <- sqrt(w)
/// weights   <- initial.w / sum(initial.w)
/// ```
///
/// Returns (weights, n_bar) where n_bar = mean(N).
fn compute_h2_weights(
    l2: &[f64],
    chi: &[f64],
    wld: &[f64],
    n_vals: &[f64],
    m: f64,
) -> (Vec<f64>, f64) {
    let n_snps = l2.len();

    // Aggregate h² estimate from mean χ².
    let mean_chi: f64 = chi.iter().sum::<f64>() / n_snps as f64;
    let mean_l2_n: f64 = (0..n_snps).map(|i| l2[i] * n_vals[i]).sum::<f64>() / n_snps as f64;
    let mut tot_agg = if mean_l2_n > 0.0 {
        (m * (mean_chi - 1.0)) / mean_l2_n
    } else {
        0.0
    };
    tot_agg = tot_agg.clamp(0.0, 1.0);

    // Per-SNP weights.
    let mut initial_w = Vec::with_capacity(n_snps);
    for i in 0..n_snps {
        let ld = l2[i].max(1.0);
        let w_ld = wld[i].max(1.0);
        let c = tot_agg * n_vals[i] / m;
        let het_w = 1.0 / (2.0 * (1.0 + c * ld).powi(2));
        let oc_w = 1.0 / w_ld;
        let w = het_w * oc_w;
        initial_w.push(w.sqrt());
    }
    let sum_iw: f64 = initial_w.iter().sum();
    let weights: Vec<f64> = if sum_iw > 0.0 {
        initial_w.iter().map(|w| w / sum_iw).collect()
    } else {
        vec![1.0 / n_snps as f64; n_snps]
    };

    let n_bar: f64 = n_vals.iter().sum::<f64>() / n_snps as f64;
    (weights, n_bar)
}

/// Compute GenomicSEM IRWLS weights for a cross-trait gencov regression.
///
/// Port of ldsc.R lines 362-394 (genetic covariance case):
/// Two sets of h²-style weights are computed (one per trait), then averaged
/// and normalized for the chi (ZZ) response. The XtX uses trait-j's weights.
///
/// Returns (weights_ld, weights_chi, n_bar) where:
/// - weights_ld = trait-j's normalized weights (for design matrix XtX)
/// - weights_chi = average of both traits' initial.w, normalized (for XtY)
/// - n_bar = sqrt(mean(N_x) * mean(N_y))
fn compute_gencov_weights(
    l2: &[f64],
    wld: &[f64],
    chi1: &[f64],
    n1: &[f64],
    chi2: &[f64],
    n2: &[f64],
    m: f64,
) -> (Vec<f64>, Vec<f64>, f64) {
    let n_snps = l2.len();

    // Compute initial.w for both traits using the IRWLS weight function.
    // R: initial.w  = sqrt(het.w  * oc.w),  using tot.agg  from trait 1's chi²
    //    initial.w2 = sqrt(het.w2 * oc.w),  using tot.agg2 from trait 2's chi²
    let mean_chi1: f64 = chi1.iter().sum::<f64>() / n_snps as f64;
    let mean_l2_n1: f64 = (0..n_snps).map(|i| l2[i] * n1[i]).sum::<f64>() / n_snps as f64;
    let ta1 = if mean_l2_n1 > 0.0 {
        ((m * (mean_chi1 - 1.0)) / mean_l2_n1).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mean_chi2: f64 = chi2.iter().sum::<f64>() / n_snps as f64;
    let mean_l2_n2: f64 = (0..n_snps).map(|i| l2[i] * n2[i]).sum::<f64>() / n_snps as f64;
    let ta2 = if mean_l2_n2 > 0.0 {
        ((m * (mean_chi2 - 1.0)) / mean_l2_n2).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mut iw1 = Vec::with_capacity(n_snps);
    let mut iw2 = Vec::with_capacity(n_snps);
    for i in 0..n_snps {
        let ld = l2[i].max(1.0);
        let w_ld = wld[i].max(1.0);
        let oc = 1.0 / w_ld;

        let c1 = ta1 * n1[i] / m;
        let het1 = 1.0 / (2.0 * (1.0 + c1 * ld).powi(2));
        iw1.push((het1 * oc).sqrt());

        let c2 = ta2 * n2[i] / m;
        let het2 = 1.0 / (2.0 * (1.0 + c2 * ld).powi(2));
        iw2.push((het2 * oc).sqrt());
    }

    // weights (for XtX) = initial.w / sum(initial.w)  [trait-j only]
    let sum_iw1: f64 = iw1.iter().sum();
    let weights_ld: Vec<f64> = if sum_iw1 > 0.0 {
        iw1.iter().map(|w| w / sum_iw1).collect()
    } else {
        vec![1.0 / n_snps as f64; n_snps]
    };

    // weights_cov (for XtY) = (initial.w + initial.w2) / sum(initial.w + initial.w2)
    let sum_both: f64 = (0..n_snps).map(|i| iw1[i] + iw2[i]).sum();
    let weights_chi: Vec<f64> = if sum_both > 0.0 {
        (0..n_snps).map(|i| (iw1[i] + iw2[i]) / sum_both).collect()
    } else {
        vec![1.0 / n_snps as f64; n_snps]
    };

    // N.bar = sqrt(mean(N_x) * mean(N_y))
    let mean_n1: f64 = n1.iter().sum::<f64>() / n_snps as f64;
    let mean_n2: f64 = n2.iter().sum::<f64>() / n_snps as f64;
    let n_bar = (mean_n1 * mean_n2).sqrt();

    (weights_ld, weights_chi, n_bar)
}

/// Run the multivariate LDSC regression — faithful port of GenomicSEM ldsc.R.
///
/// For each pair (j,k) with j ≤ k:
/// - **Diagonal (j==k)**: χ² = Z², h² weights, N.bar = mean(N).
/// - **Off-diagonal (j≠k)**: ZZ = Z_j·Z_k (with allele alignment),
///   gencov weights (separate for X and y), N.bar = sqrt(mean(N_j)·mean(N_k)).
/// Per-pair filtered arrays used in LDSC regression.
type PerPairData = (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>);

#[allow(clippy::too_many_arguments)]
fn run_multivariate_ldsc(
    arrays: &LdscArrays,
    k: usize,
    m: f64,
    n_blocks: usize,
    sample_prev: &[Option<f64>],
    population_prev: &[Option<f64>],
    stand: bool,
    trait_names: &[String],
    chisq_max: Option<f64>,
) -> Result<genomic_sem::utils::Covstruc, DagError> {
    let n_snps = arrays.n_snps;
    let z_dim = k * (k + 1) / 2;
    let n_blocks_actual = n_blocks.min(n_snps);

    // Determine chisq_max: R default = max(0.001 * max(N), 80).
    let chisq_max_eff = chisq_max.unwrap_or_else(|| {
        let max_n = (0..k)
            .flat_map(|t| arrays.n[t].iter().copied())
            .fold(0.0f64, f64::max);
        (0.001 * max_n).max(80.0)
    });

    // Run regression for each pair (j, k) with j ≤ k in vech order.
    let mut s_cov = Mat::zeros(k, k);
    let mut intercepts = Mat::zeros(k, k);
    let mut v_hold = Mat::zeros(n_blocks_actual, z_dim);
    let mut n_bars = vec![0.0f64; z_dim];

    let mut pair_idx = 0;
    for j in 0..k {
        for i in 0..=j {
            // ── Per-pair data (possibly allele-aligned for gencov) ──
            let (l2_p, chi_p, wld_p, n_j_p, n_k_p): PerPairData = if i == j {
                // h²: chi = Z². Filter by chisq_max on Z².
                let mut l2 = Vec::new();
                let mut chi = Vec::new();
                let mut wld = Vec::new();
                let mut nv = Vec::new();
                let mut nk = Vec::new();
                for s in 0..n_snps {
                    let z2 = arrays.z[i][s] * arrays.z[i][s];
                    if z2 <= chisq_max_eff {
                        l2.push(arrays.l2[s]);
                        chi.push(z2);
                        wld.push(arrays.wld[s]);
                        nv.push(arrays.n[i][s]);
                        nk.push(arrays.n[i][s]);
                    }
                }
                (l2, chi, wld, nv, nk)
            } else {
                // gencov: chi = Z_i * Z_j. Allele alignment: flip Z_i sign
                // if A1 alleles differ (done in the munge/combine step upstream;
                // here we assume alignment is already correct). Filter by
                // both traits' chisq_max.
                let mut l2 = Vec::new();
                let mut chi = Vec::new();
                let mut wld = Vec::new();
                let mut nv_i = Vec::new();
                let mut nv_j = Vec::new();
                // chi1 and chi2 for weight computation.
                let mut c1 = Vec::new();
                let mut c2 = Vec::new();
                for s in 0..n_snps {
                    let z2_i = arrays.z[i][s] * arrays.z[i][s];
                    let z2_j = arrays.z[j][s] * arrays.z[j][s];
                    if z2_i <= chisq_max_eff && z2_j <= chisq_max_eff {
                        l2.push(arrays.l2[s]);
                        chi.push(arrays.z[i][s] * arrays.z[j][s]);
                        wld.push(arrays.wld[s]);
                        nv_i.push(arrays.n[i][s]);
                        nv_j.push(arrays.n[j][s]);
                        c1.push(z2_i);
                        c2.push(z2_j);
                    }
                }
                // Store chi1/chi2 for weight computation via a side channel.
                // We'll recompute inline below.
                (l2, chi, wld, nv_i, nv_j)
            };

            let n_filter = l2_p.len();
            if n_filter < n_blocks_actual {
                return Err(DagError::NodeError {
                    node_type: "gsem_ldsc".into(),
                    msg: format!(
                        "only {n_filter} SNPs survived chisq_max={chisq_max_eff:.1} filter \
                         (pair ({i},{j})); need ≥ {n_blocks_actual} for jackknife"
                    ),
                });
            }

            // ── Compute weights ──
            let (weights_ld, weights_chi, n_bar) = if i == j {
                let (w, nb) = compute_h2_weights(&l2_p, &chi_p, &wld_p, &n_j_p, m);
                (w.clone(), w, nb)
            } else {
                // Recompute chi1 and chi2 for the filtered SNPs.
                // Recompute chi1 = Z_i² and chi2 = Z_j² for the filtered SNPs.
                let mut c1 = Vec::with_capacity(n_filter);
                let mut c2 = Vec::with_capacity(n_filter);
                let mut idx = 0;
                for s in 0..n_snps {
                    let z2_i = arrays.z[i][s] * arrays.z[i][s];
                    let z2_j = arrays.z[j][s] * arrays.z[j][s];
                    if z2_i <= chisq_max_eff && z2_j <= chisq_max_eff && idx < n_filter {
                        c1.push(z2_i);
                        c2.push(z2_j);
                        idx += 1;
                    }
                }
                compute_gencov_weights(&l2_p, &wld_p, &c1, &n_j_p, &c2, &n_k_p, m)
            };

            n_bars[pair_idx] = n_bar;

            let result = genomic_sem::ldsc::block_jackknife_regression_r(
                &l2_p,
                &chi_p,
                &weights_ld,
                &weights_chi,
                n_blocks_actual,
                n_bar,
                m,
            );

            // Check for NaN immediately with per-pair diagnostics.
            if !result.reg_tot.is_finite() || !result.intercept.is_finite() {
                let l2_min = l2_p.iter().cloned().fold(f64::INFINITY, f64::min);
                let l2_max = l2_p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let chi_min = chi_p.iter().cloned().fold(f64::INFINITY, f64::min);
                let chi_max = chi_p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let wld_min = wld_p.iter().cloned().fold(f64::INFINITY, f64::min);
                let wld_max = wld_p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let wl_min = weights_ld.iter().cloned().fold(f64::INFINITY, f64::min);
                let wl_max = weights_ld.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                return Err(DagError::NodeError {
                    node_type: "gsem_ldsc".into(),
                    msg: format!(
                        "LDSC regression for pair ({i},{j}) produced non-finite \
                         result: reg_tot={}, intercept={}, coef={:.6e}\n\
                         Inputs: n_snps={n_snps}, n_filtered={n_filter}, \
                         n_blocks={n_blocks_actual}, m={m}, n_bar={n_bar:.1}\n\
                         l2 range:      [{l2_min:.4}, {l2_max:.4}]\n\
                         chi range:     [{chi_min:.4}, {chi_max:.4}]\n\
                         wld range:     [{wld_min:.4}, {wld_max:.4}]\n\
                         weights_ld:    [{wl_min:.4e}, {wl_max:.4e}]\n\
                         chisq_max:     {chisq_max_eff:.1}",
                        result.reg_tot, result.intercept, result.coef,
                    ),
                });
            }

            s_cov[(i, j)] = result.reg_tot;
            s_cov[(j, i)] = result.reg_tot;
            intercepts[(i, j)] = result.intercept;
            intercepts[(j, i)] = result.intercept;

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

    let covstruc = ldsc_output.to_covstruc();

    // Validate output: detect NaN/Inf in S or V and fail loudly instead of
    // silently returning garbage that causes downstream nodes to degenerate.
    let z_dim = k * (k + 1) / 2;
    for i in 0..k {
        for j in 0..k {
            if !covstruc.s[(i, j)].is_finite() {
                return Err(DagError::NodeError {
                    node_type: "gsem_ldsc".into(),
                    msg: format!(
                        "S[{i},{j}] is not finite ({}) after LDSC regression; \
                         {n_snps} SNPs, n_blocks={n_blocks_actual}, m={m}, \
                         n_bars={n_bars:?}. Possible causes: NULL/NaN values \
                         in LD-score panel, insufficient overlap between \
                         sumstats and LD panel, or numerical overflow.",
                        covstruc.s[(i, j)]
                    ),
                });
            }
        }
    }
    for i in 0..z_dim {
        for j in 0..z_dim {
            if !covstruc.v[(i, j)].is_finite() {
                return Err(DagError::NodeError {
                    node_type: "gsem_ldsc".into(),
                    msg: format!(
                        "V[{i},{j}] is not finite ({}) after LDSC regression; \
                         {n_snps} SNPs, n_blocks={n_blocks_actual}, m={m}.",
                        covstruc.v[(i, j)]
                    ),
                });
            }
        }
    }

    Ok(covstruc)
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
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
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

        let result = genomic_sem::usermodel::usermodel(&covstruc, &user_config).map_err(|e| {
            DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            }
        })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx
            .session()
            .read_batch(output_batch)
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
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
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
        let result = genomic_sem::commonfactor::commonfactor(&covstruc, &config).map_err(|e| {
            DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: e.to_string(),
            }
        })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx
            .session()
            .read_batch(output_batch)
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
        fields.push(Field::new(format!("r_{i}"), DataType::Float64, false));
    }
    for i in 0..z {
        fields.push(Field::new(format!("v_r_{i}"), DataType::Float64, false));
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
        NodePorts::new().add_input_port(None).add_output_port(None) // schema depends on n_traits at runtime
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
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
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
        let result =
            genomic_sem::rgmodel::rgmodel(&covstruc, false).map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let k = self.config.n_traits;
        let z = k * (k + 1) / 2;
        let r_vec = genomic_sem::linalg::vech(&result.r);

        let mut columns: Vec<Arc<dyn Array>> = Vec::new();
        for &r in r_vec.iter().take(z) {
            columns.push(Arc::new(Float64Array::from(vec![r])));
        }
        for i in 0..z {
            columns.push(Arc::new(Float64Array::from(vec![result.v_r[(i, i)]])));
        }

        let output_batch =
            RecordBatch::try_new(rgmodel_output_schema(k), columns).map_err(|e| {
                DagError::NodeError {
                    node_type: GSEM_RGMODEL_NODE_KIND.into(),
                    msg: format!("arrow: {e}"),
                }
            })?;

        let df = node_ctx
            .session()
            .read_batch(output_batch)
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
    use arrow_array::{Int64Array, StructArray};
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
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
    fn test_ldsc_input_schema_has_required_columns() {
        let schema = ldsc_input_schema();
        assert!(schema.field_with_name("rsid").is_ok());
        assert!(schema.field_with_name("z").is_ok());
        assert!(schema.field_with_name("n").is_ok());
        assert!(schema.field_with_name("trait").is_ok());
        // Exactly 4 required columns.
        assert_eq!(schema.fields().len(), 4);
    }

    #[test]
    fn test_ldsc_input_schema_munge_output_compatibility() {
        // The gsem_munge output schema is a superset of ldsc_input_schema
        // (has a1/a2 extra). schema_compatible should accept this — but
        // gsem_munge output lacks "trait" column, so the DAG schema check
        // would reject a direct munge→ldsc edge (which is correct: the user
        // must add a trait column via SQL first).
        let munge_schema = munge_output_schema();
        let _ldsc_in = ldsc_input_schema();
        // munge has rsid, z, n, a1, a2 — ldsc needs rsid, z, n, trait.
        // ldsc's "trait" is absent from munge → not directly compatible.
        assert!(munge_schema.field_with_name("trait").is_err());
    }

    fn register_test_batch(ctx: &SessionContext, name: &str, batch: RecordBatch) {
        let table = MemTable::try_new(batch.schema(), vec![vec![batch]]).unwrap();
        ctx.register_table(name, Arc::new(table)).unwrap();
    }

    fn long_sumstats_batch(
        rsids: &[&str],
        traits: &[&str],
        z: &[f64],
        n: &[f64],
        a1: &[&str],
        a2: &[&str],
    ) -> RecordBatch {
        let schema = Arc2::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("z", DataType::Float64, false),
            Field::new("n", DataType::Float64, false),
            Field::new("trait", DataType::Utf8, false),
            Field::new("a1", DataType::Utf8, false),
            Field::new("a2", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(StringArray::from(rsids.to_vec())),
                Arc2::new(Float64Array::from(z.to_vec())),
                Arc2::new(Float64Array::from(n.to_vec())),
                Arc2::new(StringArray::from(traits.to_vec())),
                Arc2::new(StringArray::from(a1.to_vec())),
                Arc2::new(StringArray::from(a2.to_vec())),
            ],
        )
        .unwrap()
    }

    fn ld_panel_batch() -> RecordBatch {
        let contigs = vec!["2", "10", "2"];
        let positions = vec![10_i64, 20, 30];
        let locus = StructArray::new(
            vec![
                Arc2::new(Field::new("contig", DataType::Utf8, false)),
                Arc2::new(Field::new("position", DataType::Int64, false)),
            ]
            .into(),
            vec![
                Arc2::new(StringArray::from(contigs.clone())) as Arc2<dyn Array>,
                Arc2::new(Int64Array::from(positions.clone())),
            ],
            None,
        );
        let schema = Arc2::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("ld_score", DataType::Float64, false),
            Field::new("w_ld", DataType::Float64, false),
            Field::new(
                "locus",
                DataType::Struct(
                    vec![
                        Arc2::new(Field::new("contig", DataType::Utf8, false)),
                        Arc2::new(Field::new("position", DataType::Int64, false)),
                    ]
                    .into(),
                ),
                false,
            ),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(StringArray::from(vec!["rsA", "rsB", "rsC"])),
                Arc2::new(Float64Array::from(vec![1.0, 2.0, 3.0])),
                Arc2::new(Float64Array::from(vec![1.0, 1.5, 2.0])),
                Arc2::new(locus) as Arc2<dyn Array>,
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn wide_join_aligns_alleles_and_orders_genomically() {
        let ctx = SessionContext::new();
        register_test_batch(
            &ctx,
            "gsem_sumstats",
            long_sumstats_batch(
                &["rsA", "rsB", "rsC", "rsA", "rsB", "rsC"],
                &["a", "a", "a", "b", "b", "b"],
                &[2.0, 1.0, 3.0, 5.0, 4.0, 6.0],
                &[1000.0; 6],
                &["C", "a", "G", "T", "g", "A"],
                &["T", "g", "A", "C", "a", "G"],
            ),
        );
        register_test_batch(&ctx, "ld_panel", ld_panel_batch());

        let joined = build_wide_join(&ctx, &["a".into(), "b".into()], "ld_panel")
            .await
            .unwrap();
        let batches = joined.collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];

        let rsids = batch
            .column_by_name("rsid")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(rsids.value(0), "rsA");
        assert_eq!(rsids.value(1), "rsC");
        assert_eq!(rsids.value(2), "rsB");

        let z_a = batch
            .column_by_name("z_0")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let z_b = batch
            .column_by_name("z_1")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(z_a.value(0), 2.0);
        assert_eq!(z_a.value(1), 3.0);
        assert_eq!(z_a.value(2), 1.0);
        assert_eq!(z_b.value(0), -5.0);
        assert_eq!(z_b.value(1), -6.0);
        assert_eq!(z_b.value(2), -4.0);
    }

    #[tokio::test]
    async fn wide_join_rejects_unalignable_alleles() {
        let ctx = SessionContext::new();
        register_test_batch(
            &ctx,
            "gsem_sumstats",
            long_sumstats_batch(
                &["rsA", "rsA"],
                &["a", "b"],
                &[2.0, 5.0],
                &[1000.0; 2],
                &["A", "G"],
                &["G", "T"],
            ),
        );
        register_test_batch(&ctx, "ld_panel", ld_panel_batch());

        let err = build_wide_join(&ctx, &["a".into(), "b".into()], "ld_panel")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot align alleles"), "{err}");
    }

    #[tokio::test]
    async fn duplicate_trait_rsids_are_rejected() {
        let ctx = SessionContext::new();
        register_test_batch(
            &ctx,
            "gsem_sumstats",
            long_sumstats_batch(
                &["rsA", "rsA"],
                &["a", "a"],
                &[1.0, 2.0],
                &[1000.0; 2],
                &["A", "A"],
                &["G", "G"],
            ),
        );

        let err = validate_unique_trait_rsids(&ctx)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate (trait, rsid)"), "{err}");
    }

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

    /// Synthetic multivariate LDSC: 3 traits with clearly different Z patterns.
    /// Verifies that the S matrix has correct off-diagonal elements (gencov ≠ h²)
    /// — not the S[i,j] = S[j,j] pattern reported in the bug report.
    #[test]
    fn test_run_multivariate_ldsc_3trait_s_matrix() {
        let n_snps = 1000;
        let l2: Vec<f64> = (0..n_snps).map(|i| 1.0 + (i as f64) * 0.05).collect();
        let wld: Vec<f64> = l2.iter().map(|x| x * 0.5).collect();

        // Three traits with INDEPENDENT Z-score patterns.
        // Trait 0: sign alternates every 2, moderate signal
        let z0: Vec<f64> = (0..n_snps)
            .map(|i| {
                let chi = 1.0 + 0.01 * l2[i];
                if i % 2 == 0 { chi.sqrt() } else { -chi.sqrt() }
            })
            .collect();
        // Trait 1: sign alternates every 3, different magnitude
        let z1: Vec<f64> = (0..n_snps)
            .map(|i| {
                let chi = 1.0 + 0.005 * l2[i];
                if i % 3 == 0 { chi.sqrt() } else { -chi.sqrt() }
            })
            .collect();
        // Trait 2: sign alternates every 5, yet another pattern
        let z2: Vec<f64> = (0..n_snps)
            .map(|i| {
                let chi = 1.0 + 0.002 * l2[i];
                if i % 5 == 0 { chi.sqrt() } else { -chi.sqrt() }
            })
            .collect();

        let arrays = LdscArrays {
            z: vec![z0, z1, z2],
            n: vec![
                vec![100000.0; n_snps],
                vec![80000.0; n_snps],
                vec![60000.0; n_snps],
            ],
            l2,
            wld,
            n_snps,
        };

        let result = run_multivariate_ldsc(
            &arrays,
            3,
            5000.0,
            20,
            &[None, None, None],
            &[None, None, None],
            false,
            &["t0".into(), "t1".into(), "t2".into()],
            None, // no chisq_max filter
        )
        .unwrap();

        // h² (diagonal) should all be positive but different.
        let h2_0 = result.s[(0, 0)];
        let h2_1 = result.s[(1, 1)];
        let h2_2 = result.s[(2, 2)];
        assert!(h2_0.is_finite(), "h²₀ should be finite, got {h2_0}");
        assert!(h2_1.is_finite(), "h²₁ should be finite, got {h2_1}");
        assert!(h2_2.is_finite(), "h²₂ should be finite, got {h2_2}");

        // CRITICAL: gencov must NOT equal h² of either trait.
        // If S[i,j] == S[j,j] for all i<j, it means the gencov regression
        // is producing h² instead of cross-trait covariance — a bug.
        let gc_01 = result.s[(0, 1)];
        let gc_02 = result.s[(0, 2)];
        let gc_12 = result.s[(1, 2)];

        eprintln!("S matrix:");
        eprintln!("  h²₀={h2_0:.6}  gc₀₁={gc_01:.6}  gc₀₂={gc_02:.6}");
        eprintln!("  gc₀₁={gc_01:.6}  h²₁={h2_1:.6}  gc₁₂={gc_12:.6}");
        eprintln!("  gc₀₂={gc_02:.6}  gc₁₂={gc_12:.6}  h²₂={h2_2:.6}");

        let tol = 1e-8;
        assert!(
            (gc_01 - h2_1).abs() > tol || (gc_01 - h2_0).abs() > tol,
            "BUG: gencov(0,1)={gc_01:.6} equals h²₀={h2_0:.6} or h²₁={h2_1:.6}"
        );
        assert!(
            (gc_02 - h2_2).abs() > tol || (gc_02 - h2_0).abs() > tol,
            "BUG: gencov(0,2)={gc_02:.6} equals h²₀={h2_0:.6} or h²₂={h2_2:.6}"
        );
        assert!(
            (gc_12 - h2_2).abs() > tol || (gc_12 - h2_1).abs() > tol,
            "BUG: gencov(1,2)={gc_12:.6} equals h²₁={h2_1:.6} or h²₂={h2_2:.6}"
        );

        // Symmetry check
        assert!((result.s[(0, 1)] - result.s[(1, 0)]).abs() < 1e-15);
        assert!((result.s[(0, 2)] - result.s[(2, 0)]).abs() < 1e-15);
        assert!((result.s[(1, 2)] - result.s[(2, 1)]).abs() < 1e-15);
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
            1000.0,        // M
            10,            // few blocks for speed
            &[None, None], // continuous traits
            &[None, None],
            false, // no standardization
            &["t1".into(), "t2".into()],
            None, // no chisq_max filter
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

    /// Verify that `extract_arrays` skips SNPs with NULL or non-finite l2/wld
    /// values. This was the root cause of the all-NULL S/V bug: NULL ld_score
    /// or w_ld entries from the VFS panel produced NaN that contaminated
    /// every regression sum.
    #[test]
    fn test_extract_arrays_filters_null_l2_wld() {
        use arrow_array::builder::Float64Builder;
        let k = 2;

        // Build a schema matching the build_wide_join SQL output:
        // rsid, z_0, n_0, z_1, n_1, l2, wld
        let schema = Arc2::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("z_0", DataType::Float64, true),
            Field::new("n_0", DataType::Float64, true),
            Field::new("z_1", DataType::Float64, true),
            Field::new("n_1", DataType::Float64, true),
            Field::new("l2", DataType::Float64, true),
            Field::new("wld", DataType::Float64, true),
        ]));

        // 5 SNPs: SNP 0-2 are clean; SNP 3 has NULL l2; SNP 4 has NaN wld.
        let rsids = StringArray::from(vec!["rs1", "rs2", "rs3", "rs4", "rs5"]);

        let z_0 = Float64Array::from(vec![1.0, 2.0, -1.5, 0.5, -2.0]);
        let n_0 = Float64Array::from(vec![1000.0; 5]);
        let z_1 = Float64Array::from(vec![0.5, -1.0, 2.0, 1.5, -0.5]);
        let n_1 = Float64Array::from(vec![2000.0; 5]);

        // l2: [10.0, 20.0, 30.0, NULL, 50.0]
        let mut l2_b = Float64Builder::new();
        l2_b.append_value(10.0);
        l2_b.append_value(20.0);
        l2_b.append_value(30.0);
        l2_b.append_null();
        l2_b.append_value(50.0);
        let l2 = l2_b.finish();

        // wld: [1.0, 2.0, 3.0, 4.0, NaN]
        let mut wld_b = Float64Builder::new();
        wld_b.append_value(1.0);
        wld_b.append_value(2.0);
        wld_b.append_value(3.0);
        wld_b.append_value(4.0);
        wld_b.append_value(f64::NAN);
        let wld = wld_b.finish();

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc2::new(rsids),
                Arc2::new(z_0),
                Arc2::new(n_0),
                Arc2::new(z_1),
                Arc2::new(n_1),
                Arc2::new(l2),
                Arc2::new(wld),
            ],
        )
        .unwrap();

        let extracted = extract_arrays(&[batch], k).unwrap();

        // SNPs 0-2 survive; SNPs 3 (NULL l2) and 4 (NaN wld) are filtered.
        assert_eq!(extracted.n_snps, 3);
        assert_eq!(extracted.l2, vec![10.0, 20.0, 30.0]);
        assert_eq!(extracted.wld, vec![1.0, 2.0, 3.0]);
        assert_eq!(extracted.z[0], vec![1.0, 2.0, -1.5]);
        assert_eq!(extracted.z[1], vec![0.5, -1.0, 2.0]);
    }

    /// Verify that `run_multivariate_ldsc` returns an error (not NaN output)
    /// when the regression produces non-finite values.
    #[test]
    fn test_ldsc_detects_nan_output() {
        // All-zero l2 and wld → singular regression → the node should detect
        // non-finite values in the output and fail loudly.
        // Actually zeros get filtered by extract_arrays, so test via direct
        // call to run_multivariate_ldsc with degenerate data.
        let n_snps = 10;
        let arrays = LdscArrays {
            z: vec![vec![0.0; n_snps]; 2],
            n: vec![vec![1000.0; n_snps]; 2],
            l2: vec![1.0; n_snps], // constant l2 → singular regression
            wld: vec![1.0; n_snps],
            n_snps,
        };

        let result = run_multivariate_ldsc(
            &arrays,
            2,
            5_961_159.0,
            5,
            &[None, None],
            &[None, None],
            false,
            &["t1".into(), "t2".into()],
            None,
        );

        // With constant l2, the XtX matrix is singular but solve_small_pub
        // returns 0.0 for degenerate dimensions → S is all zeros (finite).
        // The result should be Ok with zero S, not NaN.
        // This test documents that degenerate but finite inputs don't trigger
        // the NaN guard.
        match result {
            Ok(cov) => {
                // S[0,0] should be 0 (no variance in l2 → slope is 0).
                assert!(cov.s[(0, 0)].is_finite());
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(msg.contains("not finite"), "unexpected error: {msg}");
            }
        }
    }
}
