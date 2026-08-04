//! LD Score Regression bivariate node — genetic correlation (rg).
//!
//! Takes **two** upstream GWAS summary-statistics `DataFrame`s (trait 1 and
//! trait 2, each with Z-scores, sample sizes, and rsid), queries the Iceberg
//! data lake for the LD score panel under `iceberg.ld_score.*`, inner-joins all
//! three on rsid so only SNPs shared by *both* traits and the panel survive,
//! and runs the bivariate LD Score Regression via [`ldsc::regress::RG::new`].
//! Outputs a single-row summary `DataFrame` with rg, its SE/z/p, the cross-trait
//! gencov, and each trait's h².
//!
//! This mirrors [`super::ldsc_hsq::LdscHsqNode`]; the only structural difference
//! is the second input port and the 3-way join producing aligned `Z1/Z2/N1/N2`
//! columns. Unlike h², rg has no DataFrame entry point in the `ldsc` crate, so
//! the node collects the joined columns into plain vectors and calls
//! [`ldsc::regress::RG::new`] directly.

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
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LdscRgNodeError {
    #[error("LDSC computation failed: {0}")]
    Ldsc(#[from] ldsc::LdscError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("datalake error: {0}")]
    Datalake(String),
}

impl From<LdscRgNodeError> for DagError {
    fn from(e: LdscRgNodeError) -> Self {
        DagError::NodeError {
            node_type: "ldsc_rg".to_string(),
            msg: e.to_string(),
        }
    }
}

impl From<datalake::error::Error> for LdscRgNodeError {
    fn from(e: datalake::error::Error) -> Self {
        LdscRgNodeError::Datalake(e.to_string())
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// The fixed output schema of the LDSC rg summary `DataFrame`.
///
/// Columns: `rg`, `rg_se`, `rg_z`, `rg_p`, `gencov`, `gencov_se`, `h2_1`,
/// `h2_1_se`, `h2_2`, `h2_2_se`, `n_snp` (all Float64).
///
/// `rg`/`rg_se`/`rg_z`/`rg_p` are `NaN` when either trait's h² ≤ 0 (rg is
/// undefined); the row is still emitted.
///
/// This is the single source of truth shared by the node's declared output port
/// (so the DAG can validate downstream edges) and [`build_result_batch`].
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("rg", DataType::Float64, false),
        Field::new("rg_se", DataType::Float64, false),
        Field::new("rg_z", DataType::Float64, false),
        Field::new("rg_p", DataType::Float64, false),
        Field::new("gencov", DataType::Float64, false),
        Field::new("gencov_se", DataType::Float64, false),
        Field::new("h2_1", DataType::Float64, false),
        Field::new("h2_1_se", DataType::Float64, false),
        Field::new("h2_2", DataType::Float64, false),
        Field::new("h2_2_se", DataType::Float64, false),
        Field::new("n_snp", DataType::Float64, false),
    ]))
}

/// The fixed input column names each upstream GWAS sumstats `DataFrame` must
/// expose. Both input ports share this schema; it is enforced by
/// [`input_schema`] so the DAG rejects mis-shaped edges at `add_edge` time.
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_RSID_COL: &str = "rsid";

/// Input port schema (shared by both ports): per-SNP Z-score (`z`, Float64),
/// sample size (`n`, Float64), rsid join key (`rsid`, Utf8). See
/// [`super::ldsc_hsq`] for the rationale behind pinning these names.
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Build a single-row rg summary `RecordBatch` from the [`ldsc::regress::RG`]
/// result.
fn build_result_batch(
    rg: &ldsc::regress::RG,
    n_snp: usize,
) -> Result<RecordBatch, LdscRgNodeError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![rg.rg_ratio])),
            Arc::new(Float64Array::from(vec![rg.rg_se])),
            Arc::new(Float64Array::from(vec![rg.z])),
            Arc::new(Float64Array::from(vec![rg.p])),
            Arc::new(Float64Array::from(vec![rg.gencov.reg.tot])),
            Arc::new(Float64Array::from(vec![rg.gencov.reg.tot_se])),
            Arc::new(Float64Array::from(vec![rg.hsq1.reg.tot])),
            Arc::new(Float64Array::from(vec![rg.hsq1.reg.tot_se])),
            Arc::new(Float64Array::from(vec![rg.hsq2.reg.tot])),
            Arc::new(Float64Array::from(vec![rg.hsq2.reg.tot_se])),
            Arc::new(Float64Array::from(vec![n_snp as f64])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the LDSC rg estimation algorithm.
///
/// Bundles the parameters that govern the bivariate LD Score Regression
/// itself.  M (the number of SNPs in the LD score annotation) is derived at
/// execution time by counting the rows in the LD score panel table.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LdscRgConfig {
    /// Number of block-jackknife blocks used by [`ldsc::regress::RG`] to
    /// compute standard errors and intercepts. A typical value is 200.
    pub n_blocks: usize,
    /// Optional fixed intercept for trait 1's h² regression. `None` lets LDSC
    /// estimate it freely (the common case).
    pub intercept_hsq1: Option<f64>,
    /// Optional fixed intercept for trait 2's h² regression. `None` lets LDSC
    /// estimate it freely (the common case).
    pub intercept_hsq2: Option<f64>,
    /// Optional fixed intercept for the cross-trait gencov regression. `None`
    /// lets LDSC estimate it freely (the common case).
    pub intercept_gencov: Option<f64>,
    /// Two-step estimator cutoff. `None` falls back to the LDSC default of 30
    /// (matching the h² node), applied only when no intercept is constrained.
    pub two_step: Option<f64>,
}

impl LdscRgConfig {
    /// Construct an [`LdscRgConfig`] with free intercepts and the default
    /// two-step cutoff.
    pub fn new(n_blocks: usize) -> Self {
        Self {
            n_blocks,
            intercept_hsq1: None,
            intercept_hsq2: None,
            intercept_gencov: None,
            two_step: None,
        }
    }
}

impl Default for LdscRgConfig {
    fn default() -> Self {
        Self {
            n_blocks: 200,
            intercept_hsq1: None,
            intercept_hsq2: None,
            intercept_gencov: None,
            two_step: None,
        }
    }
}

// =====================================================================
// Node
// =====================================================================

const LDSC_RG_NODE_KIND: &str = "ldsc_rg";

/// A transform node that runs bivariate LD Score Regression for genetic
/// correlation (rg) between two GWAS traits.
///
/// Accepts two upstream `DataFrame`s (trait 1 on port 0, trait 2 on port 1),
/// queries the Iceberg data lake for the LD score panel, inner-joins all three
/// on rsid (so only SNPs shared by both traits and the panel are used), and
/// runs the bivariate regression.
///
/// Each upstream `DataFrame` must have columns named exactly `z` (Float64),
/// `n` (Float64), and `rsid` (Utf8) — enforced by the input port schemas.
#[derive(Clone)]
pub struct LdscRgNode {
    /// DAG node metadata (id, ports): two typed inputs, one typed output.
    meta: NodePorts,
    /// Algorithm configuration; see [`LdscRgConfig`].
    ldsc_rg: LdscRgConfig,
}

pub struct LdscRgNodeFactory {}

impl NodeFactory for LdscRgNodeFactory {
    fn kind(&self) -> &'static str {
        LDSC_RG_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Bivariate LDSC for genetic correlation (rg) between two GWAS traits."
    }

    fn doc(&self) -> &'static str {
        "Bivariate LD Score Regression transform node for genetic correlation (rg) \
        estimation between two GWAS traits. Takes two upstream summary statistics \
        DataFrames (trait 1 on port 0, trait 2 on port 1, each with z, n, rsid), \
        queries the Iceberg data lake for the LD score panel, 3-way joins on rsid, \
        and runs bivariate LDSC. Outputs a single-row summary with rg, its SE/z/p, \
        cross-trait gencov, and each trait's h².\n\n\
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
        schema_for!(LdscRgConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: LdscRgConfig = serde_json::from_value(spec)?;
        let node = LdscRgNode::new(config);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let cfg = parse_spec::<LdscRgConfig>(spec, "ldsc_rg")?;
        let out = ctx.output_var.to_string();
        let input1 = ctx
            .input_vars
            .get(0)
            .cloned()
            .unwrap_or_else(|| "__missing_input_0".into());
        let input2 = ctx
            .input_vars
            .get(1)
            .cloned()
            .unwrap_or_else(|| "__missing_input_1".into());
        let s1 = ctx.fresh_var("sumstats1");
        let s2 = ctx.fresh_var("sumstats2");
        let tmp1 = ctx.fresh_var("tmp1");
        let tmp2 = ctx.fresh_var("tmp2");
        let result = ctx.fresh_var("rg_result");
        let mut intercept_flags = String::new();
        if let Some(v) = cfg.intercept_hsq1 {
            intercept_flags.push_str(&format!(" --intercept-h2 {v}"));
        }
        if let Some(v) = cfg.two_step {
            intercept_flags.push_str(&format!(" --two-step {v}"));
        }
        let code = vec![
            format!("# LDSC genetic correlation estimation"),
            format!("{s1} <- data.frame(rsid = {input1}$rsid, Z = {input1}$z, N = {input1}$n)"),
            format!("{s2} <- data.frame(rsid = {input2}$rsid, Z = {input2}$z, N = {input2}$n)"),
            format!("{tmp1} <- tempfile(fileext = \".sumstats\")"),
            format!("{tmp2} <- tempfile(fileext = \".sumstats\")"),
            format!("data.table::fwrite({s1}, {tmp1}, sep = \"\\t\")"),
            format!("data.table::fwrite({s2}, {tmp2}, sep = \"\\t\")"),
            format!("\"# NOTE: ldsc.py --rg call:\""),
            format!(
                "{result} <- system2(\"ldsc.py\", c(\"--rg\", {tmp1}, \",\", {tmp2}, \"--ref-ld\", \"baselineLD.\", \"--w-ld\", \"weights.\", \"--n-blocks\", \"{}\"{intercept_flags}), stdout = TRUE, stderr = TRUE)",
                cfg.n_blocks
            ),
            format!("cat({result}, sep = \"\\n\")"),
            format!("{out} <- list(rg = NA_real_, rg_se = NA_real_)"),
        ];
        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
    }
}

impl LdscRgNode {
    /// Construct an [`LdscRgNode`].
    ///
    /// The per-execution `SessionContext` (object-store registry + Iceberg
    /// catalog) is injected by the framework at `execute` time, so the node
    /// holds only its algorithm configuration. Both upstream `DataFrame`s
    /// must expose columns `z` (Float64), `n` (Float64), and `rsid` (Utf8) —
    /// enforced by the input port schemas.
    pub fn new(ldsc_rg: LdscRgConfig) -> Self {
        // Fixed, typed ports: two inputs (trait 1, trait 2) carrying GWAS
        // sumstats, and one output with the fixed rg summary schema. Declaring
        // the schemas lets the DAG validate edge compatibility at
        // `add_edge`/`validate` time.
        Self {
            meta: port_layout(),
            ldsc_rg,
        }
    }
}

/// Static port layout for every [`LdscRgNode`]: two typed inputs (trait 1 and
/// trait 2, both GWAS sumstats) and one typed output with the fixed rg summary
/// schema.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

/// The fixed column names produced by the internal 3-way SQL join. The SQL
/// aliases output columns to these names so the downstream vector extraction is
/// independent of the user-facing column names.
const LD_Z1_COL: &str = "z1";
const LD_Z2_COL: &str = "z2";
const LD_N1_COL: &str = "n1";
const LD_N2_COL: &str = "n2";
const LD_REF_COL: &str = "l2_0";
const LD_WLD_COL: &str = "wld";

#[async_trait]
impl DagNode for LdscRgNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "ldsc_rg"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Two inputs: trait 1 on port 0, trait 2 on port 1.
        let input1 = inputs
            .iter()
            .find(|i| i.port == 0)
            .ok_or(LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "missing trait-1 input DataFrame (port 0)".into(),
            )))?;
        let input2 = inputs
            .iter()
            .find(|i| i.port == 1)
            .ok_or(LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "missing trait-2 input DataFrame (port 1)".into(),
            )))?;

        // 1. Build an isolated DataFusion context with the Iceberg catalog
        //    registered (under "iceberg"), then delegate to the
        //    catalog-independent pipeline. Splitting here lets the pipeline be
        //    exercised end-to-end against an in-memory catalog (see
        //    `tests::run_with_test_catalog`).
        let ctx = node_ctx.session();

        // TODO: Auto select LD score panel table by population
        // TODO: Make the LD panel configurable so callers can switch between
        // panels (e.g. 1000g_eur vs ukbb_eur) without editing source.
        //
        // --- Old ukbb_eur panel (single ld_score column, no w_ld) ---
        // let (rg, n_snp) =
        //     Self::run_with_ctx(&ctx, &input1.data, &input2.data, "ukbb_eur", &self.ldsc_rg).await?;
        //
        // --- New 1000g_eur panel (ld_score + w_ld as separate columns) ---
        let (rg, n_snp) =
            Self::run_with_ctx(&ctx, &input1.data, &input2.data, "1000g_eur", &self.ldsc_rg)
                .await?;

        // 2. Build a single-row summary RecordBatch and return.
        let batch = build_result_batch(&rg, n_snp)?;
        let df = ctx.read_batch(batch).map_err(LdscRgNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

impl LdscRgNode {
    /// The catalog-independent rg pipeline.
    ///
    /// Given a [`SessionContext`] in which `iceberg.ld_score.{ld_table}` resolves
    /// to an LD-score panel, this registers the two upstream sumstats
    /// `DataFrame`s as `sumstats1` / `sumstats2`, runs the 3-way inner join on
    /// rsid (keeping only SNPs shared by both traits and the panel), collects
    /// the aligned vectors, and fits [`ldsc::regress::RG`].
    ///
    /// Returns the fitted [`ldsc::regress::RG`] and the SNP count. Extracted
    /// from [`DagNode::execute`](LdscRgNode::execute) so the full pipeline can
    /// be tested against an in-memory catalog without a live Iceberg REST
    /// server.
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        input1: &datafusion::prelude::DataFrame,
        input2: &datafusion::prelude::DataFrame,
        ld_table: &str,
        cfg: &LdscRgConfig,
    ) -> Result<(ldsc::regress::RG, usize), DagError> {
        // 1. Register both upstream sumstats DataFrames as temporary tables.
        ctx.register_table("sumstats1", input1.clone().into_view())
            .map_err(LdscRgNodeError::ReadBatch)?;
        ctx.register_table("sumstats2", input2.clone().into_view())
            .map_err(LdscRgNodeError::ReadBatch)?;

        // 2. Build SQL: 3-way inner join — both traits share an rsid that is
        //    also present in the LD score panel. The 1000g_eur panel has
        //    separate ld_score (ref LD) and w_ld (weight LD) columns.
        //    Ordered by genomic position so the block jackknife groups
        //    consecutive SNPs.
        //
        // --- Old ukbb_eur panel (ld_score used for both ref_ld and w_ld) ---
        // let sql = format!(
        //     r#"SELECT s1."{z}" AS "{Z1}", s2."{z}" AS "{Z2}",
        //               s1."{n}" AS "{N1}", s2."{n}" AS "{N2}",
        //               l.ld_score AS "{REF}", l.ld_score AS "{WLD}"
        //        FROM sumstats1 AS s1
        //        INNER JOIN sumstats2 AS s2 ON s1."{rsid}" = s2."{rsid}"
        //        INNER JOIN iceberg.ld_score.{table} AS l ON s1."{rsid}" = l.rsid
        //        ORDER BY l.locus.position"#,
        //     ... (same bind params)
        // );
        let sql = format!(
            r#"SELECT s1."{z}" AS "{Z1}", s2."{z}" AS "{Z2}",
                      s1."{n}" AS "{N1}", s2."{n}" AS "{N2}",
                      l.ld_score AS "{REF}", l.w_ld AS "{WLD}"
               FROM sumstats1 AS s1
               INNER JOIN sumstats2 AS s2 ON s1."{rsid}" = s2."{rsid}"
               INNER JOIN iceberg.ld_score."{table}" AS l ON s1."{rsid}" = l.rsid
               ORDER BY l.locus.position"#,
            z = INPUT_Z_COL,
            n = INPUT_N_COL,
            rsid = INPUT_RSID_COL,
            table = ld_table,
            Z1 = LD_Z1_COL,
            Z2 = LD_Z2_COL,
            N1 = LD_N1_COL,
            N2 = LD_N2_COL,
            REF = LD_REF_COL,
            WLD = LD_WLD_COL,
        );

        // 3. Execute the join and collect the aligned rows.
        let joined_df = ctx.sql(&sql).await.map_err(LdscRgNodeError::ReadBatch)?;
        let batches = joined_df
            .clone()
            .collect()
            .await
            .map_err(LdscRgNodeError::ReadBatch)?;
        if batches.is_empty() {
            return Err(LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "ldsc_rg: joined DataFrame is empty (no SNPs shared by both traits and the LD panel)".into(),
            ))
            .into());
        }

        // 4. Extract the aligned vectors.
        let z1 = extract_f64(&batches, LD_Z1_COL)?;
        let z2 = extract_f64(&batches, LD_Z2_COL)?;
        let n1 = extract_f64(&batches, LD_N1_COL)?;
        let n2 = extract_f64(&batches, LD_N2_COL)?;
        let ref_ld = extract_f64(&batches, LD_REF_COL)?;
        let w_ld = extract_f64(&batches, LD_WLD_COL)?;
        let n_snp = z1.len();
        if z2.len() != n_snp
            || n1.len() != n_snp
            || n2.len() != n_snp
            || ref_ld.len() != n_snp
            || w_ld.len() != n_snp
        {
            return Err(LdscRgNodeError::Ldsc(ldsc::LdscError::DimensionMismatch(format!(
                "ldsc_rg: aligned column length mismatch (z1={}, z2={}, n1={}, n2={}, ref_ld={}, w_ld={})",
                n_snp,
                z2.len(),
                n1.len(),
                n2.len(),
                ref_ld.len(),
                w_ld.len()
            )))
            .into());
        }

        // 5. Build the LD score design matrix (n_snp × 1) and weight vector.
        let x = Mat::from_fn(n_snp, 1, |i, _| ref_ld[i]);

        // 6. Resolve config, applying the LDSC two-step default of 30 when no
        //    intercept is constrained (matching the h² node).
        let LdscRgConfig {
            n_blocks,
            intercept_hsq1,
            intercept_hsq2,
            intercept_gencov,
            two_step,
        } = cfg;
        // Derive M from the companion `_m` table (per-annotation M_5_50),
        // matching the h² node and S-LDSC. Using COUNT(*) of the panel
        // overestimates M because the panel row set can differ from the
        // M_5_50 SNP set used when LD scores were computed.
        let m = super::ldsc_common::read_m_5_50(ctx, ld_table, 1)
            .await
            .map_err(|e| LdscRgNodeError::Datalake(e.to_string()))?;
        let two_step = two_step.or(
            if intercept_hsq1.is_none() && intercept_hsq2.is_none() && intercept_gencov.is_none() {
                Some(30.0)
            } else {
                None
            },
        );

        // 7. Run the bivariate regression.
        let rg = ldsc::regress::RG::new(
            &z1,
            &z2,
            &x,
            &w_ld,
            &n1,
            &n2,
            &m,
            *intercept_hsq1,
            *intercept_hsq2,
            *intercept_gencov,
            *n_blocks,
            two_step,
        )
        .map_err(LdscRgNodeError::from)?;

        Ok((rg, n_snp))
    }
}

// =====================================================================
// Column extraction
// =====================================================================

/// Extract a named numeric column from a sequence of record batches into a
/// flat `Vec<f64>`, casting nulls to `NaN`. Mirrors the downcast ladder in
/// `ldsc::ingest` (kept private to the `ldsc` crate), so the data-engine node
/// is self-contained.
fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, LdscRgNodeError> {
    let schema = batches
        .first()
        .ok_or(LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            "extract_f64: no batches".into(),
        )))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(format!(
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
        return Err(LdscRgNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            format!("column '{name}' is not numeric (got {dtype})"),
        )));
    }

    let mut out = Vec::new();
    for batch in batches {
        push_numeric(batch.column(idx), &mut out);
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

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    fn node_ctx() -> crate::node_registry::registry::NodeCtx {
        crate::node_registry::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }
    use super::*;
    use arrow_array::{Array, Int64Array, StringArray, StructArray};
    use datafusion::catalog::{
        CatalogProvider, MemTable, MemoryCatalogProvider, MemorySchemaProvider, SchemaProvider,
    };
    use datafusion::prelude::SessionContext;

    // -----------------------------------------------------------------
    // Structural / unit checks (no catalog, no execution)
    // -----------------------------------------------------------------

    /// Construct the node and assert its kind and two-input/one-output port
    /// topology.
    #[tokio::test]
    async fn test_ldsc_rg_node_structure() {
        let node = LdscRgNode::new(LdscRgConfig::new(5));
        assert_eq!(node.kind(), "ldsc_rg");
        assert_eq!(node.ports().input_ports().len(), 2);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    /// The declared output schema has exactly the rg summary columns.
    #[test]
    fn test_output_schema_matches_batch() {
        let schema = output_schema();
        assert_eq!(schema.fields().len(), 11);
        for name in [
            "rg",
            "rg_se",
            "rg_z",
            "rg_p",
            "gencov",
            "gencov_se",
            "h2_1",
            "h2_1_se",
            "h2_2",
            "h2_2_se",
            "n_snp",
        ] {
            assert!(schema.field_with_name(name).is_ok(), "missing {name}");
        }
    }

    /// `extract_f64` casts a Float64 column and turns nulls into NaN.
    #[test]
    fn test_extract_f64_casts_and_nulls() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, true),
            Field::new("rsid", DataType::Utf8, false),
        ]));
        let z = Float64Array::from(vec![Some(1.5), None, Some(-2.0)]);
        let rsid = StringArray::from(vec!["a", "b", "c"]);
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(z) as _, Arc::new(rsid) as _]).unwrap();
        let v = extract_f64(&[batch], "z").unwrap();
        assert_eq!(v.len(), 3);
        assert!((v[0] - 1.5).abs() < 1e-12);
        assert!(v[1].is_nan());
        assert!((v[2] + 2.0).abs() < 1e-12);
    }

    // -----------------------------------------------------------------
    // In-memory catalog harness
    // -----------------------------------------------------------------
    //
    // The production node resolves the LD panel through `iceberg.ld_score.*`
    // via a live REST catalog. To exercise the *full* pipeline
    // (SQL 3-way join → vector extraction → RG fit → output batch)
    // deterministically and without any external service, we register an
    // in-memory `MemoryCatalogProvider` under the same `iceberg` name, with a
    // `ld_score.1000g_eur` table backed by a `MemTable`. The node's SQL then
    // resolves identically to production.

    /// The LD-panel row count used by the synthetic fixtures.
    const N_SNP: usize = 200;

    /// Build a synthetic LD-score panel `RecordBatch` matching the `1000g_eur`
    /// panel schema: `rsid` (Utf8), `ld_score` (Float64), `w_ld` (Float64),
    /// `locus` (Struct<position: Int64>). `ld_score` strictly increases so the
    /// LDSC slope is well identified; `w_ld` mirrors `ld_score` (same values)
    /// so the analytic test fixtures remain exact; `position` increases in
    /// lock-step so the SQL `ORDER BY l.locus.position` preserves the LD order
    /// (keeping the block jackknife deterministic).
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
            Field::new("w_ld", DataType::Float64, false),
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
                Arc::new(Float64Array::from(ld.clone())),
                Arc::new(Float64Array::from(ld)),
                Arc::new(locus) as Arc<dyn Array>,
            ],
        )
        .unwrap()
    }

    /// Build a synthetic GWAS sumstats `RecordBatch` (`z`, `n`, `rsid`) for one
    /// trait, given a per-SNP Z vector.
    fn sumstats_batch(z: &[f64], rsids: &[String], n_samp: f64) -> RecordBatch {
        let n: Vec<f64> = vec![n_samp; z.len()];
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, false),
            Field::new("n", DataType::Float64, false),
            Field::new("rsid", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(z.to_vec())),
                Arc::new(Float64Array::from(n)),
                Arc::new(StringArray::from(rsids.to_vec())),
            ],
        )
        .unwrap()
    }

    /// Build a `SessionContext` with an in-memory `iceberg.ld_score.1000g_eur`
    /// table holding `ld_panel_batch(n)`, plus its `1000g_eur_m` companion
    /// (single-row M_5_50 = `n`).
    fn ctx_with_ld_panel(n: usize) -> SessionContext {
        let ctx = SessionContext::new();
        let batch = ld_panel_batch(n);

        let schema = batch.schema();
        let table = MemTable::try_new(schema, vec![vec![batch]]).unwrap();

        // Companion M table: one row, annotation "baseline", m_5_50 = n.
        let m_schema = Arc::new(Schema::new(vec![
            Field::new("annotation", DataType::Utf8, false),
            Field::new("m_5_50", DataType::Float64, false),
        ]));
        let m_batch = RecordBatch::try_new(
            m_schema,
            vec![
                Arc::new(StringArray::from(vec!["baseline"])),
                Arc::new(Float64Array::from(vec![n as f64])),
            ],
        )
        .unwrap();
        let m_table = MemTable::try_new(m_batch.schema(), vec![vec![m_batch]]).unwrap();

        let ld_schema = MemorySchemaProvider::new();
        ld_schema
            .register_table("1000g_eur".to_string(), Arc::new(table))
            .unwrap();
        ld_schema
            .register_table("1000g_eur_m".to_string(), Arc::new(m_table))
            .unwrap();
        let catalog = MemoryCatalogProvider::new();
        catalog
            .register_schema("ld_score", Arc::new(ld_schema))
            .unwrap();
        ctx.register_catalog("iceberg", Arc::new(catalog));
        ctx
    }

    /// Constrained-intercept config mirroring the proven `regress::RG` unit
    /// tests: `intercept_hsq1 = intercept_hsq2 = 1`, `intercept_gencov = 0`,
    /// no two-step. Under the analytic ±Z construction this yields rg = ∓1.
    fn constrained_cfg() -> LdscRgConfig {
        LdscRgConfig {
            n_blocks: 20,
            intercept_hsq1: Some(1.0),
            intercept_hsq2: Some(1.0),
            intercept_gencov: Some(0.0),
            two_step: None,
        }
    }

    /// Run the full pipeline against the in-memory catalog for a pair of Z
    /// vectors over the shared rsid set.
    async fn run_pipeline(
        z1: &[f64],
        z2: &[f64],
        cfg: &LdscRgConfig,
    ) -> (ldsc::regress::RG, usize) {
        let rsids: Vec<String> = (0..z1.len())
            .map(|i| format!("rs{}", 1_000_000 + i))
            .collect();
        let ctx = ctx_with_ld_panel(N_SNP);
        let df1 = ctx.read_batch(sumstats_batch(z1, &rsids, 1000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(z2, &rsids, 1000.0)).unwrap();
        LdscRgNode::run_with_ctx(&ctx, &df1, &df2, "1000g_eur", cfg)
            .await
            .expect("rg pipeline should succeed")
    }

    // -----------------------------------------------------------------
    // Known-answer end-to-end tests
    // -----------------------------------------------------------------

    /// When the two traits' Z-scores are perfect negatives (`z2 = -z1`) over a
    /// shared, well-conditioned LD panel, the cross-trait covariance is the
    /// negative of each trait's χ² signal, so the recovered rg is near −1.
    ///
    /// Drives the *full* node path (SQL 3-way join → vector extraction →
    /// `RG::new`), not just `RG` directly.
    ///
    /// Note on tolerances: `|rg|` lands at ≈0.97 rather than exactly 1 because
    /// the `Hsq` and `Gencov` IRWLS weight functions differ (null intercepts
    /// 1 vs 0, distinct variance models), so the gencov slope is a few percent
    /// smaller in magnitude than the h² slope even on identical designs. We
    /// therefore assert `|rg|≈1` loosely (0.1) but check the *exact*
    /// relationships tightly: since `z2² == z1²` the two h² fits are
    /// bit-identical (`h1 == h2`), and `rg == gencov/h1` must hold to
    /// machine precision.
    #[tokio::test]
    async fn e2e_perfect_anticorrelation_is_near_minus_one() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z1: Vec<f64> = ld.iter().map(|l| l * 10.0).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();

        let (rg, n_snp) = run_pipeline(&z1, &z2, &constrained_cfg()).await;

        assert_eq!(n_snp, N_SNP, "all shared SNPs must survive the join");
        assert!(
            rg.rg_ratio.is_finite(),
            "rg must be finite, got {}",
            rg.rg_ratio
        );

        // Exact symmetry: z2² == z1² ⇒ identical Hsq inputs ⇒ h1 == h2.
        assert!(
            rg.hsq1.reg.tot > 0.0 && rg.hsq2.reg.tot > 0.0,
            "both h² must be > 0 (got {}, {})",
            rg.hsq1.reg.tot,
            rg.hsq2.reg.tot
        );
        assert!(
            (rg.hsq1.reg.tot - rg.hsq2.reg.tot).abs() < 1e-9,
            "h1 should equal h2 (same |Z|), got {} vs {}",
            rg.hsq1.reg.tot,
            rg.hsq2.reg.tot
        );

        // Exact rg wiring: rg = gencov / sqrt(h1·h2) = gencov / h1.
        let recomputed = rg.gencov.reg.tot / rg.hsq1.reg.tot;
        assert!(
            (recomputed - rg.rg_ratio).abs() < 1e-9,
            "rg should equal gencov/h1, got rg={} gencov/h1={}",
            rg.rg_ratio,
            recomputed
        );

        // Magnitude near the −1 boundary (loose; see note on IRWLS asymmetry).
        assert!(
            rg.rg_ratio < 0.0,
            "anticorrelated ⇒ rg < 0, got {}",
            rg.rg_ratio
        );
        assert!(
            (rg.rg_ratio + 1.0).abs() < 0.1,
            "rg should be near -1, got {}",
            rg.rg_ratio
        );
        assert!(
            rg.gencov.reg.tot < 0.0,
            "gencov must be negative, got {}",
            rg.gencov.reg.tot
        );
    }

    /// Symmetric to the above: `z2 = z1` ⇒ rg near +1, gencov positive.
    #[tokio::test]
    async fn e2e_perfect_correlation_is_near_plus_one() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z1: Vec<f64> = ld.iter().map(|l| l * 10.0).collect();
        let z2 = z1.clone();

        let (rg, n_snp) = run_pipeline(&z1, &z2, &constrained_cfg()).await;
        assert_eq!(n_snp, N_SNP);
        assert!(
            rg.rg_ratio.is_finite(),
            "rg={}, expected finite",
            rg.rg_ratio
        );

        assert!(
            (rg.hsq1.reg.tot - rg.hsq2.reg.tot).abs() < 1e-9,
            "h1 should equal h2, got {} vs {}",
            rg.hsq1.reg.tot,
            rg.hsq2.reg.tot
        );
        let recomputed = rg.gencov.reg.tot / rg.hsq1.reg.tot;
        assert!(
            (recomputed - rg.rg_ratio).abs() < 1e-9,
            "rg should equal gencov/h1, got rg={} gencov/h1={}",
            rg.rg_ratio,
            recomputed
        );
        assert!(
            rg.rg_ratio > 0.0,
            "correlated ⇒ rg > 0, got {}",
            rg.rg_ratio
        );
        assert!(
            (rg.rg_ratio - 1.0).abs() < 0.1,
            "rg should be near +1, got {}",
            rg.rg_ratio
        );
        assert!(
            rg.gencov.reg.tot > 0.0,
            "gencov must be positive, got {}",
            rg.gencov.reg.tot
        );
    }

    /// The production default config (free intercepts + two-step cutoff 30)
    /// must also produce a sane, sign-correct rg on a well-conditioned signal.
    /// Uses a milder Z scale so the bulk of χ² stays below the two-step cutoff.
    #[tokio::test]
    async fn e2e_default_config_sign_correct_and_finite() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        // scale 0.2 ⇒ max χ² = 0.04 · ld² ≈ 0.04 · 436 ≈ 17.5 < 30 (two-step
        // keeps all SNPs), mean χ² ≈ 6.4 ≫ 1 (strong h² signal).
        let z1: Vec<f64> = ld.iter().map(|l| l * 0.2).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();

        let cfg = LdscRgConfig::new(20);
        assert!(cfg.two_step.is_none(), "fixture precondition");
        // run_with_ctx applies the LDSC default two_step = 30 internally.

        let (rg, n_snp) = run_pipeline(&z1, &z2, &cfg).await;
        assert_eq!(n_snp, N_SNP);
        assert!(
            rg.rg_ratio.is_finite(),
            "rg must be finite under default config, got {}",
            rg.rg_ratio
        );
        assert!(
            rg.hsq1.reg.tot > 0.0 && rg.hsq2.reg.tot > 0.0,
            "h² must be > 0, got {} / {}",
            rg.hsq1.reg.tot,
            rg.hsq2.reg.tot
        );
        assert!(
            rg.rg_ratio < 0.0,
            "anticorrelated signal ⇒ rg < 0, got {}",
            rg.rg_ratio
        );
    }

    /// `build_result_batch` wires the fitted `RG` into exactly the declared
    /// output schema (full path including output construction).
    #[tokio::test]
    async fn e2e_result_batch_has_declared_schema() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z1: Vec<f64> = ld.iter().map(|l| l * 10.0).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();
        let (rg, n_snp) = run_pipeline(&z1, &z2, &constrained_cfg()).await;

        let batch = build_result_batch(&rg, n_snp).unwrap();
        assert_eq!(batch.schema(), output_schema());
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 11);
        // n_snp column carries the joined SNP count.
        let n_snp_col = batch
            .column(10)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(n_snp_col.value(0) as usize, N_SNP);
    }

    // -----------------------------------------------------------------
    // Error-path end-to-end tests
    // -----------------------------------------------------------------

    /// When the two traits share no rsid with each other (and hence none with
    /// the panel), the 3-way inner join is empty and the pipeline must error
    /// with `InvalidInput` rather than silently producing NaNs.
    #[tokio::test]
    async fn e2e_disjoint_rsid_sets_yield_error() {
        let ctx = ctx_with_ld_panel(N_SNP);
        // Trait 1 covers rs1_000_000..rs1_000_050 (in the panel); trait 2 uses a
        // disjoint range the panel does not contain.
        let rs1: Vec<String> = (0..50).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let rs2: Vec<String> = (0..50).map(|i| format!("rs{}", 9_000_000 + i)).collect();
        let z1: Vec<f64> = (0..50).map(|i| (i as f64) * 0.1).collect();
        let z2 = z1.clone();

        let df1 = ctx.read_batch(sumstats_batch(&z1, &rs1, 1000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(&z2, &rs2, 1000.0)).unwrap();

        let res = LdscRgNode::run_with_ctx(&ctx, &df1, &df2, "1000g_eur", &constrained_cfg()).await;
        assert!(
            res.is_err(),
            "disjoint rsid sets must error, not silently return NaN"
        );
    }

    /// A missing input port must surface a clear error before any catalog work.
    #[tokio::test]
    async fn e2e_missing_input_yields_error() {
        let mut node = LdscRgNode::new(constrained_cfg());
        let batch = sumstats_batch(
            &[1.0, 2.0, 3.0],
            &["rs1".into(), "rs2".into(), "rs3".into()],
            1000.0,
        );
        let df = SessionContext::new().read_batch(batch).unwrap();
        let one_input = vec![super::super::meta::NodeInput { port: 0, data: df }];
        let res = node
            .execute(
                &node_ctx(),
                &one_input,
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await;
        assert!(res.is_err(), "missing trait-2 input must error");
    }

    /// Only SNPs present in all three tables survive: feeding traits whose rsid
    /// sets are a strict subset of the panel still yields the intersection,
    /// not the union.
    #[tokio::test]
    async fn e2e_join_keeps_intersection_only() {
        let ctx = ctx_with_ld_panel(N_SNP);
        // Trait 1 + 2 share only the first 80 rsids (subset of the 200-row panel).
        let shared: Vec<String> = (0..80).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let ld: Vec<f64> = (0..80).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z1: Vec<f64> = ld.iter().map(|l| l * 10.0).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();

        let df1 = ctx
            .read_batch(sumstats_batch(&z1, &shared, 1000.0))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch(&z2, &shared, 1000.0))
            .unwrap();
        let (rg, n_snp) =
            LdscRgNode::run_with_ctx(&ctx, &df1, &df2, "1000g_eur", &constrained_cfg())
                .await
                .expect("intersection join should succeed");
        assert_eq!(n_snp, 80, "only the 80 shared rsids survive");
        assert!(rg.rg_ratio.is_finite());
    }
}
