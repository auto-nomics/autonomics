//! MR-PRESSO node.
//!
//! Wraps the pure-Rust [`mrpresso`] crate (a faithful port of the R `MRPRESSO`
//! package, Verbanck et al. 2018). MR-PRESSO evaluates horizontal pleiotropy
//! in multi-instrument Mendelian randomisation via three components: the
//! **global test** (detection), the **outlier test** (correction via outlier
//! removal), and the **distortion test** (change in the causal estimate).
//!
//! The node takes a single upstream `DataFrame` of per-instrument (typically
//! per-SNP) summary statistics — outcome effect + SE and one or more exposure
//! effects + SEs. The column names are supplied as spec parameters (mirroring
//! R's `BetaOutcome` / `BetaExposure` / `SdOutcome` / `SdExposure` arguments).
//!
//! Because the Rust port reproduces R's exact Mersenne-Twister + inversion
//! `rnorm`, the empirical p-values match R exactly for a shared `seed`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum MrpressoNodeError {
    #[error("MR-PRESSO computation failed: {0}")]
    Mrpresso(#[from] mrpresso::MrpressoError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got {dtype})")]
    WrongColumnType { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("column length mismatch for '{name}'")]
    LengthMismatch { name: String },
}

impl ::dag_core::dag::NodeError for MrpressoNodeError {
    fn node_type(&self) -> &str {
        MRPRESSO_NODE_KIND
    }
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the MR-PRESSO node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MrpressoConfig {
    /// Column name of the outcome effect (`BetaOutcome`).
    pub beta_outcome: String,
    /// Column name of the outcome standard error (`SdOutcome`).
    pub sd_outcome: String,
    /// Column names of the exposure effects (`BetaExposure`).
    pub beta_exposure: Vec<String>,
    /// Column names of the exposure standard errors (`SdExposure`).
    pub sd_exposure: Vec<String>,
    /// Column name of the per-row instrument label (e.g. SNP rsID), used to
    /// name outlier indices. Optional.
    #[serde(default)]
    pub label_column: Option<String>,
    /// Run the outlier test (`OUTLIERtest`). Default true.
    #[serde(default = "default_true")]
    pub outlier_test: bool,
    /// Run the distortion test (`DISTORTIONtest`). Default true.
    #[serde(default = "default_true")]
    pub distortion_test: bool,
    /// Significance threshold in `(0, 1]` (`SignifThreshold`). Default 0.05.
    #[serde(default = "default_threshold")]
    pub signif_threshold: f64,
    /// Number of simulated null-distribution draws (`NbDistribution`).
    /// Default 1000.
    #[serde(default = "default_nb")]
    pub nb_distribution: usize,
    /// RNG seed (`set.seed`). Default 123 for determinism.
    #[serde(default = "default_seed")]
    pub seed: u32,
}

fn default_true() -> bool {
    true
}
fn default_threshold() -> f64 {
    0.05
}
fn default_nb() -> usize {
    1000
}
fn default_seed() -> u32 {
    123
}

impl Default for MrpressoConfig {
    fn default() -> Self {
        Self {
            beta_outcome: String::new(),
            sd_outcome: String::new(),
            beta_exposure: vec![],
            sd_exposure: vec![],
            label_column: None,
            outlier_test: true,
            distortion_test: true,
            signif_threshold: default_threshold(),
            nb_distribution: default_nb(),
            seed: default_seed(),
        }
    }
}

// =====================================================================
// Output schema
// =====================================================================

/// One long-format output table covering every MR-PRESSO result section,
/// tagged by a `section` column:
///
/// * `global` — 1 row: `pvalue`, `rss_obs`.
/// * `main_mr` — 2·p rows (raw + outlier-corrected per exposure):
///   `exposure`, `analysis`, `causal_estimate`, `sd`, `t_stat`, `pvalue`.
/// * `outlier` — n rows (when the outlier test runs): `index`, `label`,
///   `rss_obs`, `pvalue`.
/// * `distortion` — p rows (when the distortion test runs):
///   `exposure`, `distortion_coefficient`, `pvalue`, `outlier_indices`.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("section", DataType::Utf8, false),
        Field::new("exposure", DataType::Utf8, true),
        Field::new("analysis", DataType::Utf8, true),
        Field::new("index", DataType::Int64, true),
        Field::new("label", DataType::Utf8, true),
        Field::new("causal_estimate", DataType::Float64, true),
        Field::new("sd", DataType::Float64, true),
        Field::new("t_stat", DataType::Float64, true),
        Field::new("pvalue", DataType::Float64, true),
        Field::new("rss_obs", DataType::Float64, true),
        Field::new("distortion_coefficient", DataType::Float64, true),
        Field::new("outlier_indices", DataType::Utf8, true),
    ]))
}

// =====================================================================
// Column extraction
// =====================================================================

fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, MrpressoNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(MrpressoNodeError::EmptyInput)?;
    schema
        .index_of(name)
        .map_err(|_| MrpressoNodeError::MissingColumn { name: name.into() })
}

/// Extract a numeric column, casting integer/float types; null → NaN.
fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MrpressoNodeError> {
    let idx = column_index(batches, name)?;
    let mut out = Vec::new();
    for batch in batches {
        out.extend(numeric_values(batch.column(idx)));
    }
    Ok(out)
}

/// Extract a Utf8 column into `Vec<Option<String>>`.
fn extract_opt_string(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<Option<String>>, MrpressoNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    ) {
        return Err(MrpressoNodeError::WrongColumnType {
            name: name.into(),
            dtype: dtype.to_string(),
        });
    }
    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        match dtype {
            DataType::Utf8 => {
                for v in col.as_any().downcast_ref::<StringArray>().unwrap().iter() {
                    out.push(v.map(str::to_string));
                }
            }
            DataType::Utf8View => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::StringViewArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string));
                }
            }
            _ => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::LargeStringArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string));
                }
            }
        }
    }
    Ok(out)
}

/// Numeric values from a single column array (null → NaN), across all
/// physical numeric types.
fn numeric_values(col: &dyn Array) -> Vec<f64> {
    let mut out = Vec::with_capacity(col.len());
    macro_rules! cast {
        ($T:ty) => {
            if let Some(a) = col.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(x) => x as f64,
                        None => f64::NAN,
                    });
                }
                return out;
            }
        };
    }
    cast!(arrow_array::Int8Array);
    cast!(arrow_array::Int16Array);
    cast!(arrow_array::Int32Array);
    cast!(arrow_array::Int64Array);
    cast!(arrow_array::UInt8Array);
    cast!(arrow_array::UInt16Array);
    cast!(arrow_array::UInt32Array);
    cast!(arrow_array::UInt64Array);
    cast!(arrow_array::Float32Array);
    cast!(arrow_array::Float64Array);
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
    out
}

// =====================================================================
// Node
// =====================================================================

const MRPRESSO_NODE_KIND: &str = "mrpresso";

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct MrpressoNode {
    meta: NodePorts,
    config: MrpressoConfig,
}

impl MrpressoNode {
    pub fn new(config: MrpressoConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct MrpressoNodeFactory {}

impl NodeFactory for MrpressoNodeFactory {
    fn kind(&self) -> &'static str {
        MRPRESSO_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "MR-PRESSO: pleiotropy global test, outlier removal and distortion test."
    }

    fn doc(&self) -> &'static str {
        "MR-PRESSO (Mendelian Randomization Pleiotropy RESidual Sum and Outlier) \
        evaluates horizontal pleiotropy in multi-instrument Mendelian \
        randomisation from summary statistics. It returns a global pleiotropy \
        test, per-instrument outlier p-values, and a distortion test of the \
        causal estimate before/after outlier removal (Verbanck et al. 2018, \
        Nature Genetics)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MrpressoConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: MrpressoConfig = serde_json::from_value(spec)?;
        Ok(Box::new(MrpressoNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<MrpressoConfig>(spec, "mrpresso")?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let exposures = format!(
            "c({})",
            cfg.beta_exposure
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let sds = format!(
            "c({})",
            cfg.sd_exposure
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let code = vec![
            format!("# MR-PRESSO: pleiotropy global / outlier / distortion tests"),
            format!("library(MRPRESSO)"),
            format!("{out} <- mr_presso("),
            format!("  BetaOutcome = \"{}\",", cfg.beta_outcome),
            format!("  BetaExposure = {exposures},"),
            format!("  SdOutcome = \"{}\",", cfg.sd_outcome),
            format!("  SdExposure = {sds},"),
            format!("  data = as.data.frame({input}),"),
            format!("  OUTLIERtest = {},", r_bool(cfg.outlier_test)),
            format!("  DISTORTIONtest = {},", r_bool(cfg.distortion_test)),
            format!("  SignifThreshold = {},", cfg.signif_threshold),
            format!("  NbDistribution = {},", cfg.nb_distribution),
            format!("  seed = {}", cfg.seed),
            format!(")"),
            format!("print({out})"),
        ];

        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["MRPRESSO".into()]
    }
}

fn r_bool(b: bool) -> &'static str {
    if b { "TRUE" } else { "FALSE" }
}

#[async_trait]
impl DagNode for MrpressoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        MRPRESSO_NODE_KIND
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
        let input = inputs.first().ok_or(MrpressoNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: MRPRESSO_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(MrpressoNodeError::EmptyInput.into());
        }

        let p = self.config.beta_exposure.len();
        if p == 0 || self.config.sd_exposure.len() != p {
            return Err(MrpressoNodeError::LengthMismatch {
                name: "beta_exposure/sd_exposure".into(),
            }
            .into());
        }

        // Extract outcome + exposures + SEs.
        let beta_outcome = extract_f64(&batches, &self.config.beta_outcome)?;
        let sd_outcome = extract_f64(&batches, &self.config.sd_outcome)?;
        let mut beta_exposure = Vec::with_capacity(p);
        let mut sd_exposure = Vec::with_capacity(p);
        for i in 0..p {
            beta_exposure.push(extract_f64(&batches, &self.config.beta_exposure[i])?);
            sd_exposure.push(extract_f64(&batches, &self.config.sd_exposure[i])?);
        }

        let n = beta_outcome.len();
        if sd_outcome.len() != n {
            return Err(MrpressoNodeError::LengthMismatch {
                name: "sd_outcome".into(),
            }
            .into());
        }
        for (i, v) in beta_exposure.iter().enumerate() {
            if v.len() != n || sd_exposure[i].len() != n {
                return Err(MrpressoNodeError::LengthMismatch {
                    name: format!("beta_exposure/sd_exposure[{i}]"),
                }
                .into());
            }
        }

        // Optional label column.
        let mut row_labels: Option<Vec<String>> = None;
        if let Some(lbl) = &self.config.label_column {
            let v = extract_opt_string(&batches, lbl)?;
            if v.len() != n {
                return Err(MrpressoNodeError::LengthMismatch { name: lbl.clone() }.into());
            }
            row_labels = Some(v.into_iter().map(|s| s.unwrap_or_default()).collect());
        }

        let input_crate = mrpresso::MrpressoInput {
            beta_outcome,
            beta_exposure,
            sd_outcome,
            sd_exposure,
            outlier_test: self.config.outlier_test,
            distortion_test: self.config.distortion_test,
            signif_threshold: self.config.signif_threshold,
            nb_distribution: self.config.nb_distribution,
            seed: self.config.seed,
            row_labels,
        };

        let out = mrpresso::mr_presso(&input_crate).map_err(MrpressoNodeError::Mrpresso)?;

        let batch = build_result_batch(&out)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MrpressoNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the long-format output `RecordBatch` from the MR-PRESSO result.
fn build_result_batch(out: &mrpresso::MrpressoOutput) -> Result<RecordBatch, MrpressoNodeError> {
    let mut section: Vec<&str> = Vec::new();
    let mut exposure: Vec<Option<String>> = Vec::new();
    let mut analysis: Vec<Option<String>> = Vec::new();
    let mut index: Vec<Option<i64>> = Vec::new();
    let mut label: Vec<Option<String>> = Vec::new();
    let mut causal_estimate: Vec<Option<f64>> = Vec::new();
    let mut sd: Vec<Option<f64>> = Vec::new();
    let mut t_stat: Vec<Option<f64>> = Vec::new();
    let mut pvalue: Vec<Option<f64>> = Vec::new();
    let mut rss_obs: Vec<Option<f64>> = Vec::new();
    let mut distortion_coefficient: Vec<Option<f64>> = Vec::new();
    let mut outlier_indices: Vec<Option<String>> = Vec::new();

    // Global test.
    section.push("global");
    exposure.push(None);
    analysis.push(None);
    index.push(None);
    label.push(None);
    causal_estimate.push(None);
    sd.push(None);
    t_stat.push(None);
    pvalue.push(Some(out.global.pvalue));
    rss_obs.push(Some(out.global.rss_obs));
    distortion_coefficient.push(None);
    outlier_indices.push(None);

    // Main MR results.
    for row in &out.main_mr {
        section.push("main_mr");
        exposure.push(Some(format!("E{}", row.exposure + 1)));
        analysis.push(Some(row.analysis.to_string()));
        index.push(None);
        label.push(None);
        causal_estimate.push(finite(row.causal_estimate));
        sd.push(finite(row.sd));
        t_stat.push(finite(row.t_stat));
        pvalue.push(finite(row.p_value));
        rss_obs.push(None);
        distortion_coefficient.push(None);
        outlier_indices.push(None);
    }

    // Outlier test.
    if let Some(rows) = &out.outlier {
        for r in rows {
            section.push("outlier");
            exposure.push(None);
            analysis.push(None);
            index.push(Some(r.index as i64));
            label.push(Some(r.label.clone()));
            causal_estimate.push(None);
            sd.push(None);
            t_stat.push(None);
            pvalue.push(Some(r.pvalue));
            rss_obs.push(Some(r.rss_obs));
            distortion_coefficient.push(None);
            outlier_indices.push(None);
        }
    }

    // Distortion test.
    if let Some(dt) = &out.distortion {
        let indices_str = dt
            .outlier_indices
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        for (e, coef) in dt.coefficient.iter().enumerate() {
            section.push("distortion");
            exposure.push(Some(format!("E{}", e + 1)));
            analysis.push(None);
            index.push(None);
            label.push(None);
            causal_estimate.push(None);
            sd.push(None);
            t_stat.push(None);
            pvalue.push(dt.pvalue);
            rss_obs.push(None);
            distortion_coefficient.push(Some(*coef));
            outlier_indices.push(Some(indices_str.clone()));
        }
    }

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(section)),
            Arc::new(StringArray::from(exposure)),
            Arc::new(StringArray::from(analysis)),
            Arc::new(Int64Array::from(index)),
            Arc::new(StringArray::from(label)),
            Arc::new(Float64Array::from(causal_estimate)),
            Arc::new(Float64Array::from(sd)),
            Arc::new(Float64Array::from(t_stat)),
            Arc::new(Float64Array::from(pvalue)),
            Arc::new(Float64Array::from(rss_obs)),
            Arc::new(Float64Array::from(distortion_coefficient)),
            Arc::new(StringArray::from(outlier_indices)),
        ],
    )?;
    Ok(batch)
}

/// Convert an f64 to `Option<f64>`, mapping NaN → None (arrow null).
fn finite(x: f64) -> Option<f64> {
    if x.is_nan() { None } else { Some(x) }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
            resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(std::path::PathBuf::from("."))),
            global_sem: None,
        }
    }

    /// Build a small MR-PRESSO input batch: 8 instruments, one exposure.
    fn make_input_batch() -> RecordBatch {
        let snp = StringArray::from(vec!["rs1", "rs2", "rs3", "rs4", "rs5", "rs6", "rs7", "rs8"]);
        let beta_out = Float64Array::from(vec![0.10, 0.12, 0.09, 0.15, 0.20, 0.14, 0.25, 0.30]);
        let se_out = Float64Array::from(vec![0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02]);
        let beta_exp = Float64Array::from(vec![0.20, 0.22, 0.19, 0.30, 0.40, 0.28, 0.50, 0.60]);
        let se_exp = Float64Array::from(vec![0.03, 0.03, 0.03, 0.03, 0.03, 0.03, 0.03, 0.03]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("snp", DataType::Utf8, false),
            Field::new("beta_outcome", DataType::Float64, false),
            Field::new("se_outcome", DataType::Float64, false),
            Field::new("beta_exposure", DataType::Float64, false),
            Field::new("se_exposure", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(snp),
                Arc::new(beta_out),
                Arc::new(se_out),
                Arc::new(beta_exp),
                Arc::new(se_exp),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn runs_and_emits_all_sections() {
        let mut node = MrpressoNode::new(MrpressoConfig {
            beta_outcome: "beta_outcome".into(),
            sd_outcome: "se_outcome".into(),
            beta_exposure: vec!["beta_exposure".into()],
            sd_exposure: vec!["se_exposure".into()],
            label_column: Some("snp".into()),
            outlier_test: true,
            distortion_test: true,
            signif_threshold: 0.05,
            nb_distribution: 200,
            seed: 123,
        });

        let batch = make_input_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let input = NodeInput { port: 0, data: df };

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let outputs = res.get(&0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        assert!(batch.num_rows() >= 1);
        // Column 0 is the section column.
        let section = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let mut has_global = false;
        let mut has_main = false;
        for i in 0..section.len() {
            match section.value(i) {
                "global" => has_global = true,
                "main_mr" => has_main = true,
                _ => {}
            }
        }
        assert!(has_global, "missing global section");
        assert!(has_main, "missing main_mr section");
    }
}
