//! MTAG (Multi-Trait Analysis of GWAS) node.
//!
//! Takes **two** upstream GWAS summary-statistics `DataFrame`s (trait 1 and
//! trait 2, each with Z-scores, sample sizes, allele frequencies, and rsid),
//! queries the VFS-mounted reference dataset for the LD score panel under
//! `vfs.ld_score.*`, runs:
//!
//! 1. **Σ estimation** — bivariate LDSC regression to get the residual
//!    covariance intercepts (Σ[i,j] = gencov/h² intercept).
//! 2. **Ω estimation** — GMM (method of moments) estimator of the genetic
//!    effect-size covariance.
//! 3. **MTAG analysis** — conditional-expectation formula producing adjusted
//!    betas and SEs per SNP per trait.
//!
//! Outputs **two** DataFrames (port 0 = trait 1 results, port 1 = trait 2
//! results), each with per-SNP `rsid`, `z`, `n`, `mtag_beta`, `mtag_se`,
//! `mtag_z`, `mtag_pval`.
//!
//! This mirrors [`super::ldsc_rg::LdscRgNode`] for the LD score panel access
//! and Σ estimation, then adds the MTAG-specific Ω estimation and analysis.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use statrs::distribution::ContinuousCDF;
use thiserror::Error;

use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum MtagNodeError {
    #[error("MTAG computation failed: {0}")]
    Mtag(#[from] mtag::MtagError),
    #[error("LDSC computation failed: {0}")]
    Ldsc(#[from] ldsc::LdscError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("reference data error: {0}")]
    ReferenceData(String),
}

impl ::dag_core::dag::NodeError for MtagNodeError {
    fn node_type(&self) -> &str {
        "mtag"
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// Input column names each upstream GWAS sumstats `DataFrame` must expose.
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_FRQ_COL: &str = "frq";
const INPUT_RSID_COL: &str = "rsid";

/// Output column names for each per-trait result DataFrame.
const OUT_RSID_COL: &str = "rsid";
const OUT_Z_COL: &str = "z";
const OUT_N_COL: &str = "n";
const OUT_BETA_COL: &str = "mtag_beta";
const OUT_SE_COL: &str = "mtag_se";
const OUT_ZSCORE_COL: &str = "mtag_z";
const OUT_PVAL_COL: &str = "mtag_pval";

/// Input port schema: per-SNP Z-score, sample size, allele frequency, rsid.
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_FRQ_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Output port schema: per-SNP MTAG results.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(OUT_RSID_COL, DataType::Utf8, false),
        Field::new(OUT_Z_COL, DataType::Float64, false),
        Field::new(OUT_N_COL, DataType::Float64, false),
        Field::new(OUT_BETA_COL, DataType::Float64, false),
        Field::new(OUT_SE_COL, DataType::Float64, false),
        Field::new(OUT_ZSCORE_COL, DataType::Float64, false),
        Field::new(OUT_PVAL_COL, DataType::Float64, false),
    ]))
}

/// Build a per-trait result RecordBatch from aligned vectors.
fn build_trait_batch(
    rsids: &[String],
    zs: &[f64],
    ns: &[f64],
    freqs: &[f64],
    mtag_betas: &[f64], // standardised-scale betas (from mtag_analysis)
    mtag_ses: &[f64],
    std_betas: bool,
) -> Result<RecordBatch, MtagNodeError> {
    let m = rsids.len();
    let mut beta_out = Vec::with_capacity(m);
    let mut se_out = Vec::with_capacity(m);
    let mut z_out = Vec::with_capacity(m);
    let mut pval_out = Vec::with_capacity(m);

    let n_dist = statrs::distribution::Normal::new(0.0, 1.0).unwrap();

    for i in 0..m {
        let weight = if std_betas {
            1.0
        } else {
            (2.0 * freqs[i] * (1.0 - freqs[i])).sqrt()
        };
        let beta = mtag_betas[i] / weight;
        let se = mtag_ses[i] / weight;
        let z = mtag_betas[i] / mtag_ses[i];
        let pval = 2.0 * n_dist.sf(z.abs());

        beta_out.push(beta);
        se_out.push(se);
        z_out.push(z);
        pval_out.push(pval);
    }

    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(arrow_array::StringArray::from(rsids.to_vec())),
            Arc::new(Float64Array::from(zs.to_vec())),
            Arc::new(Float64Array::from(ns.to_vec())),
            Arc::new(Float64Array::from(beta_out)),
            Arc::new(Float64Array::from(se_out)),
            Arc::new(Float64Array::from(z_out)),
            Arc::new(Float64Array::from(pval_out)),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the MTAG analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MtagConfig {
    /// Number of block-jackknife blocks for LDSC Σ estimation.
    pub n_blocks: usize,
    /// Use numerical MLE for Ω instead of the default GMM estimator.
    #[serde(default)]
    pub numerical_omega: bool,
    /// Assume perfect genetic covariance (off-diagonals = sqrt(h²_i · h²_j)).
    #[serde(default)]
    pub perfect_gencov: bool,
    /// Assume equal heritability across traits (requires perfect_gencov).
    #[serde(default)]
    pub equal_h2: bool,
    /// Output standardised betas (skip 2p(1-p) unstandardisation).
    #[serde(default)]
    pub std_betas: bool,
    /// Tolerance for numerical Ω optimisation.
    #[serde(default = "default_tol")]
    pub tol: f64,
    /// Optional fixed intercept for trait 1's h² regression. `None` = free.
    #[serde(default)]
    pub intercept_hsq1: Option<f64>,
    /// Optional fixed intercept for trait 2's h² regression. `None` = free.
    #[serde(default)]
    pub intercept_hsq2: Option<f64>,
    /// Optional fixed intercept for the cross-trait gencov regression.
    /// `None` = free.
    #[serde(default)]
    pub intercept_gencov: Option<f64>,
}

fn default_tol() -> f64 {
    1e-6
}

impl Default for MtagConfig {
    fn default() -> Self {
        Self {
            n_blocks: 200,
            numerical_omega: false,
            perfect_gencov: false,
            equal_h2: false,
            std_betas: false,
            tol: 1e-6,
            intercept_hsq1: None,
            intercept_hsq2: None,
            intercept_gencov: None,
        }
    }
}

// =====================================================================
// Node
// =====================================================================

const MTAG_NODE_KIND: &str = "mtag";

/// A transform node that runs MTAG (Multi-Trait Analysis of GWAS) on two
/// traits.
///
/// Accepts two upstream `DataFrame`s (trait 1 on port 0, trait 2 on port 1),
/// each with columns `z`, `n`, `frq`, `rsid`. Queries the VFS-mounted reference dataset
/// for the LD score panel, runs LDSC bivariate regression for Σ estimation,
/// GMM for Ω estimation, and the MTAG conditional formula. Outputs per-trait
/// result `DataFrame`s (port 0 = trait 1, port 1 = trait 2).
#[derive(Clone)]
pub struct MtagNode {
    meta: NodePorts,
    config: MtagConfig,
    ld_panel: DataBundle,
}

pub struct MtagNodeFactory {}

impl NodeFactory for MtagNodeFactory {
    fn kind(&self) -> &'static str {
        MTAG_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Multi-Trait Analysis of GWAS (MTAG) for two traits."
    }

    fn doc(&self) -> &'static str {
        "MTAG (Multi-Trait Analysis of GWAS) transform node. Takes two upstream \
        summary statistics DataFrames (trait 1 on port 0, trait 2 on port 1, each \
        with z, n, frq, rsid), queries the VFS-mounted reference dataset for the LD score \
        panel, runs LDSC bivariate regression for residual covariance (Sigma) \
        estimation, GMM for genetic covariance (Omega) estimation, and the MTAG \
        conditional-expectation formula. Outputs two per-trait result DataFrames \
        (port 0 = trait 1, port 1 = trait 2) with mtag_beta, mtag_se, mtag_z, \
        mtag_pval per SNP.\n\n\
        IMPORTANT — you MUST perform the following quality control on the \
        input GWAS summary statistics BEFORE feeding them into this node:\n\
        1. Remove palindromic SNPs (alleles that are complementary on the two \
        strands, e.g. A/T or C/G) to avoid strand-alignment ambiguity.\n\
        2. Remove duplicate SNPs so each rsid appears at most once.\n\
        3. Keep only biallelic SNPs (exactly two alleles per variant).\n\
        4. Remove SNPs on sex chromosomes (X, Y, MT) — retain autosomal SNPs \
        only.\n\
        5. Filter by minor allele frequency: keep only SNPs with MAF > 0.01 \
        (1%)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MtagConfig)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        vec![DataBundleBinding::new(
            "ld_panel",
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_UKBB_EUR,
        )]
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: MtagConfig = serde_json::from_value(spec)?;
        let node = MtagNode::new(config, node_ctx.bound_data_bundle("ld_panel")?.clone());
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<MtagConfig>(spec, "mtag")?;
        let out = ctx.output_var.to_string();
        let n_traits = ctx.input_vars.len();
        let _input = ctx
            .input_vars
            .first()
            .cloned()
            .unwrap_or_else(|| "__missing_input".into());
        let tmp_dir = ctx.fresh_var("mtag_dir");
        let result = ctx.fresh_var("mtag_result");
        let mut flags = String::new();
        if cfg.numerical_omega {
            flags.push_str(" --omega-num");
        }
        if cfg.perfect_gencov {
            flags.push_str(" --perfect-gencov");
        }
        if cfg.equal_h2 {
            flags.push_str(" --equal-h2");
        }
        if cfg.std_betas {
            flags.push_str(" --std-betas");
        }
        let code = vec![
            format!("# Multi-Trait Analysis of GWAS (MTAG) — {n_traits} trait(s)"),
            format!("# NOTE: MTAG is a Python package; this generates the CLI call"),
            format!("# Each input sumstats must be written to a temp file first"),
            format!("{tmp_dir} <- tempfile()"),
            format!("dir.create({tmp_dir})"),
            format!("# Write each trait's sumstats to {tmp_dir}/trait_N.txt"),
            format!(
                "{result} <- system2(\"python\", c(\"-m\", \"mtag\", \"--input\", {tmp_dir}, \"--output\", {tmp_dir}, \"--n-blocks\", \"{}\"{flags}), stdout = TRUE, stderr = TRUE)",
                cfg.n_blocks
            ),
            format!("cat({result}, sep = \"\\n\")"),
            format!("# NOTE: Read MTAG output files from {tmp_dir}"),
            format!("{out} <- list()"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
    }
}

impl MtagNode {
    pub fn new(config: MtagConfig, ld_panel: DataBundle) -> Self {
        Self {
            meta: port_layout(),
            config,
            ld_panel,
        }
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
        .add_output_port(Some(output_schema()))
}

/// Internal column names from the 3-way SQL join.
const LD_Z1_COL: &str = "z1";
const LD_Z2_COL: &str = "z2";
const LD_N1_COL: &str = "n1";
const LD_N2_COL: &str = "n2";
const LD_FRQ1_COL: &str = "frq1";
const LD_FRQ2_COL: &str = "frq2";
const LD_RSID_COL: &str = "rsid_joined";
const LD_REF_COL: &str = "l2_0";
const LD_WLD_COL: &str = "wld";

#[async_trait]
impl DagNode for MtagNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        MTAG_NODE_KIND
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
        let input1 = inputs
            .iter()
            .find(|i| i.port == 0)
            .ok_or(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(
                "missing trait-1 input DataFrame (port 0)".into(),
            )))?;
        let input2 = inputs
            .iter()
            .find(|i| i.port == 1)
            .ok_or(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(
                "missing trait-2 input DataFrame (port 1)".into(),
            )))?;

        let ctx = node_ctx.session();

        nodes_ldsc::ldsc_common::register_listing_table(
            &ctx,
            "ld_panel",
            &nodes_ldsc::ldsc_common::storage_url(&self.ld_panel),
        )
        .await
        .map_err(|e| MtagNodeError::ReferenceData(e.to_string()))?;

        // Run the full pipeline.
        let (batch1, batch2) = Self::run_with_ctx(
            &ctx,
            input1.dataframe()?,
            input2.dataframe()?,
            "ld_panel",
            &self.config,
        )
        .await?;

        let df1 = ctx.read_batch(batch1).map_err(MtagNodeError::ReadBatch)?;
        let df2 = ctx.read_batch(batch2).map_err(MtagNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df1);
        res.insert(1, df2);
        Ok(res)
    }
}

impl MtagNode {
    /// The catalog-independent MTAG pipeline.
    ///
    /// Given a [`SessionContext`] in which `vfs.ld_score.{ld_table}` resolves
    /// to an LD-score panel, this:
    /// 1. Registers the two upstream sumstats as temp tables.
    /// 2. 3-way inner joins on rsid with the LD panel.
    /// 3. Runs LDSC bivariate regression for Σ estimation.
    /// 4. Runs GMM for Ω estimation.
    /// 5. Runs the MTAG conditional formula.
    /// 6. Builds per-trait result batches.
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        input1: &datafusion::prelude::DataFrame,
        input2: &datafusion::prelude::DataFrame,
        panel_table: &str,
        cfg: &MtagConfig,
    ) -> Result<(RecordBatch, RecordBatch), DagError> {
        // 1. Register both upstream sumstats DataFrames as temporary tables.
        ctx.register_table("sumstats1", input1.clone().into_view())
            .map_err(MtagNodeError::ReadBatch)?;
        ctx.register_table("sumstats2", input2.clone().into_view())
            .map_err(MtagNodeError::ReadBatch)?;

        // 2. Build SQL: 3-way inner join.
        let ld_table = nodes_ldsc::ldsc_common::quote_table(panel_table);
        let sql = format!(
            r#"SELECT s1."{z}" AS "{Z1}", s2."{z}" AS "{Z2}",
                      s1."{n}" AS "{N1}", s2."{n}" AS "{N2}",
                      s1."{frq}" AS "{FRQ1}", s2."{frq}" AS "{FRQ2}",
                      s1."{rsid}" AS "{RSID_J}",
                      l.ld_score AS "{REF}", l.ld_score AS "{WLD}"
               FROM sumstats1 AS s1
               INNER JOIN sumstats2 AS s2 ON s1."{rsid}" = s2."{rsid}"
               INNER JOIN {ld_table} AS l ON s1."{rsid}" = l.rsid
               ORDER BY l.locus.position"#,
            z = INPUT_Z_COL,
            n = INPUT_N_COL,
            frq = INPUT_FRQ_COL,
            rsid = INPUT_RSID_COL,
            Z1 = LD_Z1_COL,
            Z2 = LD_Z2_COL,
            N1 = LD_N1_COL,
            N2 = LD_N2_COL,
            FRQ1 = LD_FRQ1_COL,
            FRQ2 = LD_FRQ2_COL,
            RSID_J = LD_RSID_COL,
            REF = LD_REF_COL,
            WLD = LD_WLD_COL,
        );

        let joined_df = ctx.sql(&sql).await.map_err(MtagNodeError::ReadBatch)?;
        let batches = joined_df
            .clone()
            .collect()
            .await
            .map_err(MtagNodeError::ReadBatch)?;

        if batches.is_empty() {
            return Err(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(
                "mtag: joined DataFrame is empty (no SNPs shared by both traits and the LD panel)"
                    .into(),
            ))
            .into());
        }

        // 3. Extract aligned vectors.
        let z1 = extract_f64(&batches, LD_Z1_COL)?;
        let z2 = extract_f64(&batches, LD_Z2_COL)?;
        let n1 = extract_f64(&batches, LD_N1_COL)?;
        let n2 = extract_f64(&batches, LD_N2_COL)?;
        let frq1 = extract_f64(&batches, LD_FRQ1_COL)?;
        let frq2 = extract_f64(&batches, LD_FRQ2_COL)?;
        let ref_ld = extract_f64(&batches, LD_REF_COL)?;
        let w_ld = extract_f64(&batches, LD_WLD_COL)?;
        let rsids = extract_string(&batches, LD_RSID_COL)?;
        let n_snp = z1.len();

        // 4. Derive M from the LD score panel.
        let m = vec![count_panel_snp(ctx, panel_table).await? as f64];

        // 5. LDSC bivariate regression for Σ estimation.
        let x = Mat::from_fn(n_snp, 1, |i, _| ref_ld[i]);
        let two_step = if cfg.intercept_hsq1.is_none()
            && cfg.intercept_hsq2.is_none()
            && cfg.intercept_gencov.is_none()
        {
            Some(30.0_f64)
        } else {
            None
        };

        let rg = ldsc::regress::RG::new(
            &z1,
            &z2,
            &x,
            &w_ld,
            &n1,
            &n2,
            &m,
            cfg.intercept_hsq1,
            cfg.intercept_hsq2,
            cfg.intercept_gencov,
            cfg.n_blocks,
            two_step,
        )
        .map_err(MtagNodeError::from)?;

        // Σ matrix from LDSC intercepts.
        let sigma_00 = rg.hsq1.reg.intercept.unwrap_or(1.0);
        let sigma_11 = rg.hsq2.reg.intercept.unwrap_or(1.0);
        let sigma_01 = rg.gencov.reg.intercept.unwrap_or(0.0);
        let mut sigma_hat = Mat::from_fn(2, 2, |i, j| match (i, j) {
            (0, 0) => sigma_00,
            (1, 1) => sigma_11,
            _ => sigma_01,
        });

        // Positive-definiteness adjustment.
        sigma_hat =
            mtag::linalg::pos_def_adjustment(sigma_hat, 0.99, 1000).map_err(MtagNodeError::from)?;

        // 6. Build Z and N matrices for Ω estimation and MTAG analysis.
        let mut zs = Mat::zeros(n_snp, 2);
        let mut ns = Mat::zeros(n_snp, 2);
        for i in 0..n_snp {
            zs[(i, 0)] = z1[i];
            zs[(i, 1)] = z2[i];
            ns[(i, 0)] = n1[i];
            ns[(i, 1)] = n2[i];
        }

        // 7. Ω estimation.
        let omega_cfg = mtag::omega::OmegaConfig {
            numerical: cfg.numerical_omega,
            perfect_gencov: cfg.perfect_gencov,
            equal_h2: cfg.equal_h2,
            tol: cfg.tol,
            no_overlap: false,
        };
        let omega_hat = mtag::omega::estimate_omega(&zs, &ns, &sigma_hat, &omega_cfg)
            .map_err(MtagNodeError::from)?;

        // 8. MTAG analysis.
        let result = mtag::mtag::mtag_analysis(&zs, &ns, &omega_hat, &sigma_hat);

        // 9. Build per-trait result batches.
        let mtag_betas_t1: Vec<f64> = (0..n_snp).map(|i| result.mtag_betas[(i, 0)]).collect();
        let mtag_ses_t1: Vec<f64> = (0..n_snp).map(|i| result.mtag_se[(i, 0)]).collect();
        let mtag_betas_t2: Vec<f64> = (0..n_snp).map(|i| result.mtag_betas[(i, 1)]).collect();
        let mtag_ses_t2: Vec<f64> = (0..n_snp).map(|i| result.mtag_se[(i, 1)]).collect();

        let batch1 = build_trait_batch(
            &rsids,
            &z1,
            &n1,
            &frq1,
            &mtag_betas_t1,
            &mtag_ses_t1,
            cfg.std_betas,
        )?;
        let batch2 = build_trait_batch(
            &rsids,
            &z2,
            &n2,
            &frq2,
            &mtag_betas_t2,
            &mtag_ses_t2,
            cfg.std_betas,
        )?;

        Ok((batch1, batch2))
    }
}

// =====================================================================
// Column extraction helpers (mirrors ldsc_rg.rs)
// =====================================================================

fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MtagNodeError> {
    let schema = batches
        .first()
        .ok_or(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(
            "extract_f64: no batches".into(),
        )))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MtagNodeError::Mtag(mtag::MtagError::InvalidInput(format!(
            "missing column '{name}'"
        )))
    })?;
    let dtype = schema.field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    ) {
        return Err(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(format!(
            "column '{name}' is not numeric (got {dtype})"
        ))));
    }

    let mut out = Vec::new();
    for batch in batches {
        push_numeric(batch.column(idx), &mut out);
    }
    Ok(out)
}

fn extract_string(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, MtagNodeError> {
    let schema = batches
        .first()
        .ok_or(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(
            "extract_string: no batches".into(),
        )))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MtagNodeError::Mtag(mtag::MtagError::InvalidInput(format!(
            "missing column '{name}'"
        )))
    })?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::LargeStringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringViewArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else {
            return Err(MtagNodeError::Mtag(mtag::MtagError::InvalidInput(format!(
                "column '{name}' is not a string type"
            ))));
        }
    }
    Ok(out)
}

fn push_numeric(col: &dyn Array, out: &mut Vec<f64>) {
    macro_rules! cast {
        ($arr:expr, $t:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$t>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(val) => val as f64,
                        None => f64::NAN,
                    });
                }
                return;
            }
        };
    }
    cast!(col, Int8Array);
    cast!(col, Int16Array);
    cast!(col, Int32Array);
    cast!(col, Int64Array);
    cast!(col, UInt8Array);
    cast!(col, UInt16Array);
    cast!(col, UInt32Array);
    cast!(col, UInt64Array);
    cast!(col, Float32Array);
    cast!(col, Float64Array);
}

async fn count_panel_snp(
    ctx: &datafusion::prelude::SessionContext,
    ld_table_sql: &str,
) -> Result<usize, MtagNodeError> {
    let sql = format!(
        r#"SELECT COUNT(*) AS "n" FROM {}"#,
        nodes_ldsc::ldsc_common::quote_table(ld_table_sql)
    );
    let df = ctx.sql(&sql).await.map_err(MtagNodeError::ReadBatch)?;
    let batches = df.collect().await.map_err(MtagNodeError::ReadBatch)?;
    let batch = batches
        .first()
        .ok_or(MtagNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            "count_panel_snp: no batches returned".into(),
        )))?;
    let idx = batch.schema().index_of("n").map_err(|_| {
        MtagNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            "count_panel_snp: missing column 'n'".into(),
        ))
    })?;
    let col = batch.column(idx);
    let dtype = col.data_type();
    let n = match dtype {
        DataType::UInt64 => col.as_any().downcast_ref::<UInt64Array>().unwrap().value(0) as usize,
        DataType::Int64 => col.as_any().downcast_ref::<Int64Array>().unwrap().value(0) as usize,
        _ => {
            return Err(MtagNodeError::Ldsc(ldsc::LdscError::InvalidInput(format!(
                "count_panel_snp: unsupported dtype {dtype}"
            ))));
        }
    };
    Ok(n)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    fn bundle() -> DataBundle {
        DataBundle::new("ukbb", "UKBB LD panel", "/panels/ukbb.parquet")
    }

    use super::*;
    use arrow_array::{Array, Int64Array, StringArray, StructArray};
    use datafusion::catalog::{
        CatalogProvider, MemTable, MemoryCatalogProvider, MemorySchemaProvider, SchemaProvider,
    };
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[test]
    fn test_mtag_node_structure() {
        let node = MtagNode::new(MtagConfig::default(), bundle());
        assert_eq!(node.kind(), "mtag");
        assert_eq!(node.ports().input_ports().len(), 2);
        assert_eq!(node.ports().output_ports().len(), 2);
    }

    const N_SNP: usize = 200;

    fn ld_panel_batch(n: usize) -> RecordBatch {
        let rsids: Vec<String> = (0..n).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let ld: Vec<f64> = (0..n).map(|i| 1.0 + 0.1 * i as f64).collect();
        let pos: Vec<i64> = (0..n).map(|i| i as i64).collect();

        let position_field = Arc::new(Field::new("position", DataType::Int64, false));
        let locus = StructArray::new(
            vec![position_field].into(),
            vec![Arc::new(Int64Array::from(pos)) as Arc<dyn Array>],
            None,
        );

        let schema = Arc::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("ld_score", DataType::Float64, false),
            Field::new(
                "locus",
                DataType::Struct(
                    vec![Arc::new(Field::new("position", DataType::Int64, false))].into(),
                ),
                false,
            ),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(rsids)),
                Arc::new(Float64Array::from(ld)),
                Arc::new(locus) as Arc<dyn Array>,
            ],
        )
        .unwrap()
    }

    fn sumstats_batch(z: &[f64], rsids: &[String], n_samp: f64, freq: f64) -> RecordBatch {
        let n: Vec<f64> = vec![n_samp; z.len()];
        let frq: Vec<f64> = vec![freq; z.len()];
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, false),
            Field::new("n", DataType::Float64, false),
            Field::new("frq", DataType::Float64, false),
            Field::new("rsid", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(z.to_vec())),
                Arc::new(Float64Array::from(n)),
                Arc::new(Float64Array::from(frq)),
                Arc::new(StringArray::from(rsids.to_vec())),
            ],
        )
        .unwrap()
    }

    fn ctx_with_ld_panel(n: usize) -> SessionContext {
        let ctx = SessionContext::new();
        let batch = ld_panel_batch(n);
        let schema = batch.schema();
        let table = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        ctx.register_table("ukbb_eur", Arc::new(table)).unwrap();
        ctx
    }

    #[tokio::test]
    async fn e2e_mtag_produces_finite_results() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        // Scale 0.2 ⇒ max chi² ≈ 17.5 < 30 (two-step keeps all SNPs), mean chi² ≈ 6 ≫ 1.
        let z1: Vec<f64> = ld.iter().map(|l| l * 0.2).collect();
        let z2: Vec<f64> = z1.iter().map(|z| z * 0.8).collect();
        let rsids: Vec<String> = (0..N_SNP).map(|i| format!("rs{}", 1_000_000 + i)).collect();

        let ctx = ctx_with_ld_panel(N_SNP);
        let df1 = ctx
            .read_batch(sumstats_batch(&z1, &rsids, 1000.0, 0.3))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch(&z2, &rsids, 1000.0, 0.3))
            .unwrap();

        let cfg = MtagConfig {
            n_blocks: 20,
            // Constrain intercepts to theoretical null (Sigma = I) so the
            // test exercises the Ω + mtag_analysis pipeline without depending
            // on LDSC free-intercept estimation quality on synthetic data.
            intercept_hsq1: Some(1.0),
            intercept_hsq2: Some(1.0),
            intercept_gencov: Some(0.0),
            ..Default::default()
        };

        let (batch1, batch2) = MtagNode::run_with_ctx(&ctx, &df1, &df2, "ukbb_eur", &cfg)
            .await
            .expect("MTAG pipeline should succeed");

        assert_eq!(batch1.num_rows(), N_SNP);
        assert_eq!(batch2.num_rows(), N_SNP);
        assert_eq!(batch1.num_columns(), 7);
        assert_eq!(batch2.num_columns(), 7);

        // All betas and SEs should be finite.
        let beta1 = batch1
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let se1 = batch1
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..N_SNP {
            if !beta1.value(i).is_finite() {
                eprintln!("beta1[{i}] = {} (not finite)", beta1.value(i));
            }
            if !se1.value(i).is_finite() {
                eprintln!("se1[{i}] = {} (not finite)", se1.value(i));
            }
        }
        // All betas and SEs should be finite and positive.
        for i in 0..N_SNP {
            assert!(beta1.value(i).is_finite(), "beta1[{i}] not finite");
            assert!(se1.value(i).is_finite(), "se1[{i}] not finite");
            assert!(se1.value(i) > 0.0, "se1[{i}] should be positive");
        }
    }

    /// MTAG should improve power: the mean MTAG chi² (= mean mtag_z²) should
    /// exceed the mean input chi² for correlated traits, because MTAG borrows
    /// strength across traits.
    #[tokio::test]
    async fn e2e_mtag_improves_chi2() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z1: Vec<f64> = ld.iter().map(|l| l * 0.2).collect();
        let z2: Vec<f64> = z1.iter().map(|z| z * 0.9).collect(); // highly correlated
        let rsids: Vec<String> = (0..N_SNP).map(|i| format!("rs{}", 1_000_000 + i)).collect();

        let ctx = ctx_with_ld_panel(N_SNP);
        let df1 = ctx
            .read_batch(sumstats_batch(&z1, &rsids, 1000.0, 0.3))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch(&z2, &rsids, 1000.0, 0.3))
            .unwrap();

        let cfg = MtagConfig {
            n_blocks: 20,
            intercept_hsq1: Some(1.0),
            intercept_hsq2: Some(1.0),
            intercept_gencov: Some(0.0),
            ..Default::default()
        };

        let (batch1, _batch2) = MtagNode::run_with_ctx(&ctx, &df1, &df2, "ukbb_eur", &cfg)
            .await
            .expect("MTAG pipeline should succeed");

        // Mean input chi² for trait 1.
        let mean_input_chi2: f64 = z1.iter().map(|z| z * z).sum::<f64>() / N_SNP as f64;

        // Mean MTAG chi² for trait 1.
        let mtag_z = batch1
            .column(5)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let mean_mtag_chi2: f64 =
            (0..N_SNP).map(|i| mtag_z.value(i).powi(2)).sum::<f64>() / N_SNP as f64;

        eprintln!("mean input chi² = {mean_input_chi2:.4}, mean MTAG chi² = {mean_mtag_chi2:.4}");

        // MTAG should improve (or at least not decrease) power for correlated traits.
        assert!(
            mean_mtag_chi2 >= mean_input_chi2,
            "MTAG should improve mean chi²: got {} < {}",
            mean_mtag_chi2,
            mean_input_chi2
        );
    }

    #[tokio::test]
    async fn e2e_mtag_missing_input_yields_error() {
        let mut node = MtagNode::new(MtagConfig::default(), bundle());
        let batch = sumstats_batch(
            &[1.0, 2.0, 3.0],
            &["rs1".into(), "rs2".into(), "rs3".into()],
            1000.0,
            0.3,
        );
        let df = SessionContext::new().read_batch(batch).unwrap();
        let one_input = vec![dag_core::node::NodeInput::new_dataframe(0, df)];
        let res = node
            .execute(
                &node_ctx(),
                &one_input,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await;
        assert!(res.is_err(), "missing trait-2 input must error");
    }
}
