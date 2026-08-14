//! Stratified LD Score Regression (S-LDSC) transform node.
//!
//! Takes a single upstream GWAS summary statistics `DataFrame` (with Z-scores,
//! sample sizes, and rsid), reads **multi-annotation** reference LD Scores from
//! files (e.g. baselineLD v2.2, 97 annotations), reads per-annotation M from
//! the companion `.l2.M_5_50` files, and runs stratified LD Score Regression
//! via [`ldsc::sumstats::estimate_sldsc`].
//!
//! Outputs a per-annotation result `DataFrame` — one row per annotation with
//! Category / Prop._SNPs / Coefficient / Coefficient_SE / Coefficient_z /
//! Coefficient_p / Prop._h2 / Prop._h2_SE / Enrichment / Enrichment_SE /
//! Enrichment_p — mirroring the Python LDSC `.results` table.
//!
//! Unlike [`super::ldsc_hsq`] which reads LD scores from the VFS lake
//! (single-column panel), this node reads multi-column baseline-LD from files
//! because the lake does not yet have a multi-annotation panel table.
//!
//! # Reference panel data
//!
//! The baselineLD v2.2 reference panel (1000G EUR, 97 annotations) is archived
//! at `aliyun:autonomics-data/ldsc/s-ldsc-ref/`. Restore with:
//!
//! ```bash
//! rclone copy aliyun:autonomics-data/ldsc/s-ldsc-ref/ reference/ldsc_data/ -P
//! ```
//!
//! After restore, the file prefixes for the config are:
//! - `ref_ld_chr`: `reference/ldsc_data/baselineLD.`
//! - `w_ld_chr`: `reference/ldsc_data/weights.hm3_noMHC.`

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::NodeFactory,
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LdscSldscNodeError {
    #[error("S-LDSC computation failed: {0}")]
    Ldsc(#[from] ldsc::LdscError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("reference data error: {0}")]
    ReferenceData(String),
    #[error("not yet implemented: {0}")]
    Unimplemented(String),
}

impl ::dag_core::dag::NodeError for LdscSldscNodeError {
    fn node_type(&self) -> &str {
        "sldsc"
    }
}

// =====================================================================
// Output schema
// =====================================================================

/// The fixed output schema of the S-LDSC per-annotation result `DataFrame`.
///
/// One row per annotation. Columns mirror the Python LDSC `.results` table
/// (with two-sided p-values appended):
///
/// `category` | `m_prop` | `coef` | `coef_se` | `coef_z` | `coef_p` |
/// `cat`(Prop._h2) | `cat_se` | `enrichment` | `enrichment_se` | `enrichment_p`
///
/// This is the single source of truth shared by the node's declared output
/// port (so the DAG can validate downstream edges) and [`build_result_batch`].
pub fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("category", DataType::Utf8, false),
        Field::new("m_prop", DataType::Float64, false),
        Field::new("coef", DataType::Float64, false),
        Field::new("coef_se", DataType::Float64, false),
        Field::new("coef_z", DataType::Float64, false),
        Field::new("coef_p", DataType::Float64, false),
        Field::new("cat", DataType::Float64, false),
        Field::new("cat_se", DataType::Float64, false),
        Field::new("enrichment", DataType::Float64, false),
        Field::new("enrichment_se", DataType::Float64, false),
        Field::new("enrichment_p", DataType::Float64, false),
    ]))
}

/// The fixed input column names for the upstream GWAS sumstats `DataFrame`
/// (same convention as [`super::ldsc_hsq`]).
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_RSID_COL: &str = "rsid";

/// Build the input port schema (same as ldsc_hsq).
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Build a per-annotation result `RecordBatch` from [`ldsc::sldsc::SldscResults`].
///
/// Each annotation becomes one row. This is shared by `execute()` and tests.
pub fn build_result_batch(
    r: &ldsc::sldsc::SldscResults,
) -> Result<RecordBatch, LdscSldscNodeError> {
    let schema = output_schema();

    let category: Vec<&str> = r.annotations.iter().map(|a| a.category.as_str()).collect();
    let m_prop: Vec<f64> = r.annotations.iter().map(|a| a.m_prop).collect();
    let coef: Vec<f64> = r.annotations.iter().map(|a| a.coef).collect();
    let coef_se: Vec<f64> = r.annotations.iter().map(|a| a.coef_se).collect();
    let coef_z: Vec<f64> = r.annotations.iter().map(|a| a.coef_z).collect();
    let coef_p: Vec<f64> = r.annotations.iter().map(|a| a.coef_p).collect();
    let cat: Vec<f64> = r.annotations.iter().map(|a| a.cat).collect();
    let cat_se: Vec<f64> = r.annotations.iter().map(|a| a.cat_se).collect();
    let enrichment: Vec<f64> = r.annotations.iter().map(|a| a.enrichment).collect();
    let enrichment_se: Vec<f64> = r.annotations.iter().map(|a| a.enrichment_se).collect();
    let enrichment_p: Vec<f64> = r.annotations.iter().map(|a| a.enrichment_p).collect();

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(category)),
            Arc::new(Float64Array::from(m_prop)),
            Arc::new(Float64Array::from(coef)),
            Arc::new(Float64Array::from(coef_se)),
            Arc::new(Float64Array::from(coef_z)),
            Arc::new(Float64Array::from(coef_p)),
            Arc::new(Float64Array::from(cat)),
            Arc::new(Float64Array::from(cat_se)),
            Arc::new(Float64Array::from(enrichment)),
            Arc::new(Float64Array::from(enrichment_se)),
            Arc::new(Float64Array::from(enrichment_p)),
        ],
    )?;

    Ok(batch)
}

// =====================================================================
// Config
// =====================================================================

/// VFS table name for the baselineLD v2.2 panel (hardcoded).

/// Configuration for the S-LDSC node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LdscSldscConfig {
    /// Number of block-jackknife blocks for standard errors. Typical: 200.
    #[serde(default = "default_n_blocks")]
    pub n_blocks: usize,
    /// Optional fixed intercept. `None` (default) lets the regression estimate
    /// the intercept freely, where it absorbs confounding such as population
    /// stratification.
    #[serde(default)]
    pub intercept: Option<f64>,
}

fn default_n_blocks() -> usize {
    200
}

impl LdscSldscConfig {
    pub fn new() -> Self {
        Self {
            n_blocks: 200,
            intercept: None,
        }
    }
}

impl Default for LdscSldscConfig {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
// Node + Factory
// =====================================================================

/// A transform node that runs stratified LD Score Regression (S-LDSC).
///
/// Accepts raw GWAS summary statistics as input (`z`, `n`, `rsid` columns),
/// reads multi-annotation reference LD Scores + per-annotation M from files,
/// and runs S-LDSC. Outputs a per-annotation result table.
#[derive(Clone)]
pub struct LdscSldscNode {
    meta: NodePorts,
    config: LdscSldscConfig,
}

pub struct LdscSldscNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for LdscSldscNodeFactory {
    fn kind(&self) -> &'static str {
        "sldsc"
    }

    fn desc(&self) -> &'static str {
        "Stratified LD Score Regression (S-LDSC) for partitioned heritability."
    }

    fn doc(&self) -> &'static str {
        "S-LDSC transform node for partitioning SNP-heritability across \
        functional annotations. Takes a single upstream GWAS summary \
        statistics DataFrame (z, n, rsid), reads multi-annotation reference \
        LD Scores + per-annotation M from files (baselineLD prefix), and runs \
        stratified LD Score Regression. Outputs a per-annotation result table \
        with Coefficient, Prop._h2, Enrichment, and their SE / z / p."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LdscSldscConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: LdscSldscConfig = serde_json::from_value(spec)?;
        let _ = &node_ctx; // ctx injected at execute() time
        let node = LdscSldscNode::new(config);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<LdscSldscConfig>(spec, "sldsc")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let s = ctx.fresh_var("sumstats");
        let tmp = ctx.fresh_var("tmp_file");
        let result = ctx.fresh_var("sldsc_result");
        let intercept_flag = cfg
            .intercept
            .map(|v| format!(" --intercept-h2 {v}"))
            .unwrap_or_default();
        let code = vec![
            format!("# Stratified LD Score Regression"),
            format!("{s} <- data.frame(rsid = {input}$rsid, Z = {input}$z, N = {input}$n)"),
            format!("{tmp} <- tempfile(fileext = \".sumstats\")"),
            format!("data.table::fwrite({s}, {tmp}, sep = \"\\t\")"),
            format!("# NOTE: S-LDSC requires stratified LD score files per annotation"),
            format!(
                "{result} <- system2(\"ldsc.py\", c(\"--h2\", {tmp}, \"--ref-ld\", \"baselineLD.\", \"--w-ld\", \"weights.\", \"--n-blocks\", \"{}\"{intercept_flag}), stdout = TRUE, stderr = TRUE)",
                cfg.n_blocks
            ),
            format!("cat({result}, sep = \"\\n\")"),
            format!("{out} <- list(coef = NA, coef_se = NA)"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
    }
}

impl LdscSldscNode {
    /// Construct an [`LdscSldscNode`].
    pub fn new(config: LdscSldscConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

#[async_trait]
impl DagNode for LdscSldscNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "sldsc"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input =
            inputs
                .first()
                .ok_or(LdscSldscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                    "no input DataFrame".into(),
                )))?;

        let session = ctx.session();

        crate::ldsc_common::register_listing_table(
            &session,
            "ld_panel",
            crate::ldsc_common::VFS_LDSCORE_BASELINELD_V2_2_EUR,
        )
        .await
        .map_err(|e| LdscSldscNodeError::ReferenceData(e.to_string()))?;
        crate::ldsc_common::register_listing_table(
            &session,
            "ld_panel_m",
            crate::ldsc_common::VFS_LDSCORE_BASELINELD_V2_2_EUR_M,
        )
        .await
        .map_err(|e| LdscSldscNodeError::ReferenceData(e.to_string()))?;

        let result = Self::run_with_ctx(
            &session,
            &input.data,
            "ld_panel",
            "ld_panel_m",
            &self.config,
        )
        .await?;

        let batch = build_result_batch(&result)?;
        let df = session
            .read_batch(batch)
            .map_err(LdscSldscNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

impl LdscSldscNode {
    /// The S-LDSC pipeline: query VFS for multi-annotation LD scores + M,
    /// join with upstream sumstats, run the regression, derive per-annotation
    /// results.
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        input: &datafusion::prelude::DataFrame,
        panel_table: &str,
        m_table: &str,
        cfg: &LdscSldscConfig,
    ) -> Result<ldsc::sldsc::SldscResults, LdscSldscNodeError> {
        // 1. Register upstream sumstats.
        ctx.register_table("sumstats", input.clone().into_view())
            .map_err(LdscSldscNodeError::ReadBatch)?;

        // 2. Query M table for annotation names + M values.
        let m_sql = format!(
            r#"SELECT "annotation", "m_5_50" FROM {}"#,
            crate::ldsc_common::quote_table(m_table)
        );
        let m_df = ctx
            .sql(&m_sql)
            .await
            .map_err(LdscSldscNodeError::ReadBatch)?;
        let m_batches = m_df
            .collect()
            .await
            .map_err(LdscSldscNodeError::ReadBatch)?;

        // Extract annotation names + M values.
        let (annot_names, m_values): (Vec<String>, Vec<f64>) = {
            let mut names = Vec::new();
            let mut values = Vec::new();
            for batch in &m_batches {
                let name_vals = dag_core::node::string_opt_values(batch.column(0).as_ref()).ok_or(
                    LdscSldscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                        "M table 'annotation' column is not a string type".into(),
                    )),
                )?;
                let val_col = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<arrow_array::Float64Array>()
                    .ok_or(LdscSldscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                        "M table 'm_5_50' column is not Float64".into(),
                    )))?;
                for (i, name) in name_vals.iter().enumerate().take(batch.num_rows()) {
                    names.push(name.clone().unwrap_or_default());
                    values.push(val_col.value(i));
                }
            }
            (names, values)
        };
        let n_annot = annot_names.len();
        if n_annot == 0 {
            return Err(LdscSldscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "M table is empty".into(),
            )));
        }

        // 3. Build SQL: join sumstats with LD panel on rsid, selecting
        //    z, n, all annotation columns, w_ld — ordered by genomic position.
        let select_cols: Vec<String> = annot_names
            .iter()
            .map(|c| format!(r#"l."{c}" AS "l2_{c}""#))
            .collect();
        let sql = format!(
            r#"SELECT s."z" AS "z", s."n" AS "n", {cols}, l."w_ld" AS "wld"
               FROM sumstats AS s
               INNER JOIN {ld_table} AS l
               ON s."rsid" = l."rsid"
               ORDER BY l."locus"."position""#,
            cols = select_cols.join(", "),
            ld_table = crate::ldsc_common::quote_table(panel_table),
        );

        let joined_df = ctx.sql(&sql).await.map_err(LdscSldscNodeError::ReadBatch)?;

        // 4. Extract arrays via the ldsc ingest helper.
        let ref_ld_names: Vec<String> = annot_names.iter().map(|c| format!("l2_{c}")).collect();
        let ref_ld_refs: Vec<&str> = ref_ld_names.iter().map(|s| s.as_str()).collect();
        let cols = ldsc::hsq::HsqColumns {
            snp: "",
            z: "z",
            n: "n",
            ref_ld: ref_ld_refs,
            w_ld: "wld",
        };
        let arrays = ldsc::ingest::to_arrays(joined_df, &cols).await?;
        let n_snp = arrays.z.len();
        if n_snp == 0 {
            return Err(LdscSldscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "After merging sumstats with LD panel, 0 SNPs remain.".into(),
            )));
        }

        // 5. Run the S-LDSC regression.
        let chisq: Vec<f64> = arrays.z.iter().map(|z| z * z).collect();
        let n_blocks = cfg.n_blocks.min(n_snp);
        let old_weights = n_annot > 1;
        let two_step = if n_annot == 1 && cfg.intercept.is_none() {
            Some(30.0)
        } else {
            None
        };
        let hsq = ldsc::regress::Hsq::new(
            &chisq,
            &arrays.ref_ld,
            &arrays.w_ld,
            &arrays.n,
            &m_values,
            n_blocks,
            cfg.intercept,
            two_step,
            old_weights,
        )?;

        // 6. Derive per-annotation results.
        Ok(ldsc::sldsc::build_sldsc_results(&hsq, &annot_names, n_snp))
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct the node and assert its kind and topology.
    #[tokio::test]
    async fn test_sldsc_node_structure() {
        let node = LdscSldscNode::new(LdscSldscConfig::new());
        assert_eq!(node.kind(), "sldsc");
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    /// The declared output schema has exactly the per-annotation columns.
    #[test]
    fn test_output_schema_has_all_columns() {
        let schema = output_schema();
        for name in [
            "category",
            "m_prop",
            "coef",
            "coef_se",
            "coef_z",
            "coef_p",
            "cat",
            "cat_se",
            "enrichment",
            "enrichment_se",
            "enrichment_p",
        ] {
            assert!(schema.field_with_name(name).is_ok(), "missing {name}");
        }
    }

    /// `run_with_ctx` is
    /// error (either file-not-found or Unimplemented) rather than panic.
    #[tokio::test]
    async fn test_run_with_ctx_errors_without_catalog() {
        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx
            .read_batch(arrow_array::RecordBatch::new_empty(input_schema()))
            .unwrap();
        let res = LdscSldscNode::run_with_ctx(
            &ctx,
            &df,
            "baselineLD_v2_2_eur",
            "baselineLD_v2_2_eur_m",
            &LdscSldscConfig::new(),
        )
        .await;
        assert!(res.is_err(), "should error without VFS catalog");
    }

    /// `build_result_batch` wires a `SldscResults` into the declared output
    /// schema. Uses synthetic data since `run_with_ctx` is stubbed.
    #[test]
    fn test_build_result_batch_has_declared_schema() {
        let results = ldsc::sldsc::SldscResults {
            annotations: vec![ldsc::sldsc::SldscAnnotResult {
                category: "base".into(),
                m_prop: 0.5,
                coef: 1e-8,
                coef_se: 5e-9,
                coef_z: 2.0,
                coef_p: 0.046,
                cat: 0.1,
                cat_se: 0.05,
                enrichment: 1.5,
                enrichment_se: 0.3,
                enrichment_p: 0.095,
            }],
            tot: 0.2,
            tot_se: 0.01,
            intercept: Some(1.05),
            intercept_se: Some(0.03),
            ratio: Some(0.05),
            ratio_se: Some(0.01),
            mean_chisq: 1.2,
            lambda_gc: 1.1,
            n_snp: 100000,
            n_annot: 1,
        };
        let batch = build_result_batch(&results).unwrap();
        assert_eq!(batch.schema(), output_schema());
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 11);
    }
}
