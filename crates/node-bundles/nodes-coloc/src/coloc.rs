//! Colocalisation analysis node (`coloc_abf`).
//!
//! Wraps the pure-Rust [`coloc`] crate (a faithful port of the R `coloc`
//! package, Wallace & Giambartolomei). Computes posterior probabilities of
//! five colocalisation hypotheses (H0–H4) from two GWAS summary-statistic
//! datasets sharing SNPs at a locus.
//!
//! The node accepts a single upstream `DataFrame` containing columns from
//! both datasets, merged by SNP. The config specifies which columns belong
//! to each dataset and their types. Two paths are supported per dataset:
//!
//! * **beta / varbeta** — exact ABF from effect estimates (preferred).
//! * **pvalues / MAF / N** — ABF approximated from p-values + MAF.
//!
//! Output is a long-format table: one summary row with the five posterior
//! probabilities, followed by per-SNP rows with lABF and SNP.PP.H4.

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
pub enum ColocNodeError {
    #[error("coloc computation failed: {0}")]
    Coloc(#[from] coloc::ColocError),
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
    #[error("SNP column '{0}' not found")]
    MissingSnpCol(String),
}

impl ::dag_core::dag::NodeError for ColocNodeError {
    fn node_type(&self) -> &str {
        COLOC_ABF_NODE_KIND
    }
}

// =====================================================================
// Config
// =====================================================================

/// Dataset specification for one trait.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DatasetSpec {
    /// Trait type: "quant" or "cc".
    pub r#type: String,
    /// Column name for SNP identifiers.
    pub snp: String,
    /// Column name for regression coefficients (preferred path).
    #[serde(default)]
    pub beta: Option<String>,
    /// Column name for variance of beta.
    #[serde(default)]
    pub varbeta: Option<String>,
    /// Column name for p-values (alternative path).
    #[serde(default)]
    pub pvalues: Option<String>,
    /// Column name for minor allele frequency.
    #[serde(default)]
    pub maf: Option<String>,
    /// Sample size (scalar). Required for p-value path.
    #[serde(default)]
    pub n: Option<f64>,
    /// For case–control: proportion of cases. Required for cc + p-value path.
    #[serde(default)]
    pub s: Option<f64>,
    /// For quantitative: known population SD of trait.
    #[serde(default)]
    pub sd_y: Option<f64>,
    /// Column name for genomic positions (optional).
    #[serde(default)]
    pub position: Option<String>,
}

/// Configuration for the coloc.abf node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ColocAbfConfig {
    /// Dataset 1 specification.
    pub dataset1: DatasetSpec,
    /// Dataset 2 specification.
    pub dataset2: DatasetSpec,
    /// Prior probability a SNP is associated with trait 1. Default 1e-4.
    #[serde(default = "default_p1")]
    pub p1: f64,
    /// Prior probability a SNP is associated with trait 2. Default 1e-4.
    #[serde(default = "default_p2")]
    pub p2: f64,
    /// Prior probability a SNP is associated with both traits. Default 1e-5.
    #[serde(default = "default_p12")]
    pub p12: f64,
    /// Optional common MAF column to use for both datasets.
    #[serde(default)]
    pub maf: Option<String>,
    /// Column names for per-SNP prior weights for trait 1 (optional).
    #[serde(default)]
    pub prior_weights1: Option<String>,
    /// Column names for per-SNP prior weights for trait 2 (optional).
    #[serde(default)]
    pub prior_weights2: Option<String>,
}

fn default_p1() -> f64 {
    1e-4
}
fn default_p2() -> f64 {
    1e-4
}
fn default_p12() -> f64 {
    1e-5
}

// =====================================================================
// Output schema
// =====================================================================

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("section", DataType::Utf8, false),
        Field::new("nsnps", DataType::Int64, true),
        Field::new("PP.H0.abf", DataType::Float64, true),
        Field::new("PP.H1.abf", DataType::Float64, true),
        Field::new("PP.H2.abf", DataType::Float64, true),
        Field::new("PP.H3.abf", DataType::Float64, true),
        Field::new("PP.H4.abf", DataType::Float64, true),
        Field::new("snp", DataType::Utf8, true),
        Field::new("lABF.df1", DataType::Float64, true),
        Field::new("lABF.df2", DataType::Float64, true),
        Field::new("SNP.PP.H4", DataType::Float64, true),
    ]))
}

// =====================================================================
// Column extraction
// =====================================================================

fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, ColocNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(ColocNodeError::EmptyInput)?;
    schema
        .index_of(name)
        .map_err(|_| ColocNodeError::MissingColumn { name: name.into() })
}

fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, ColocNodeError> {
    let idx = column_index(batches, name)?;
    let mut out = Vec::new();
    for batch in batches {
        out.extend(numeric_values(batch.column(idx)));
    }
    Ok(out)
}

fn extract_opt_f64(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Option<Vec<f64>>, ColocNodeError> {
    match column_index(batches, name) {
        Ok(idx) => {
            let mut out = Vec::new();
            for batch in batches {
                out.extend(numeric_values(batch.column(idx)));
            }
            Ok(Some(out))
        }
        Err(_) => Ok(None),
    }
}

fn extract_string(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, ColocNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    ) {
        return Err(ColocNodeError::WrongColumnType {
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
                    out.push(v.map(str::to_string).unwrap_or_default());
                }
            }
            DataType::Utf8View => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::StringViewArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string).unwrap_or_default());
                }
            }
            _ => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::LargeStringArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string).unwrap_or_default());
                }
            }
        }
    }
    Ok(out)
}

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

const COLOC_ABF_NODE_KIND: &str = "coloc_abf";

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct ColocAbfNode {
    meta: NodePorts,
    config: ColocAbfConfig,
}

impl ColocAbfNode {
    pub fn new(config: ColocAbfConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct ColocAbfNodeFactory {}

impl NodeFactory for ColocAbfNodeFactory {
    fn kind(&self) -> &'static str {
        COLOC_ABF_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Colocalisation analysis (coloc.abf): posterior probabilities for H0–H4."
    }

    fn doc(&self) -> &'static str {
        "Colocalisation analysis using Bayes factors (coloc.abf, Wallace & \
        Giambartolomei 2014, PLoS Genetics). Computes posterior probabilities \
        of five hypotheses: H0 (no association), H1 (trait 1 only), H2 (trait \
        2 only), H3 (both traits, different variants), H4 (both traits, shared \
        variant). Requires two GWAS datasets sharing SNPs at a locus."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ColocAbfConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: ColocAbfConfig = serde_json::from_value(spec)?;
        Ok(Box::new(ColocAbfNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<ColocAbfConfig>(spec, "coloc_abf")?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        // Build dataset1 R list.
        let d1 = &cfg.dataset1;
        let mut d1_args = vec![
            format!("snp = as.character({input}${})", r_str(&d1.snp)),
            format!("type = {}", r_str(&d1.r#type)),
        ];
        if let Some(c) = &d1.beta {
            d1_args.push(format!("beta = {input}${c}"));
        }
        if let Some(c) = &d1.varbeta {
            d1_args.push(format!("varbeta = {input}${c}"));
        }
        if let Some(c) = &d1.pvalues {
            d1_args.push(format!("pvalues = {input}${c}"));
        }
        if let Some(c) = &d1.maf {
            d1_args.push(format!("MAF = {input}${c}"));
        }
        if let Some(n) = d1.n {
            d1_args.push(format!("N = {n}"));
        }
        if let Some(s) = d1.s {
            d1_args.push(format!("s = {s}"));
        }
        if let Some(sdy) = d1.sd_y {
            d1_args.push(format!("sdY = {sdy}"));
        }
        if let Some(c) = &d1.position {
            d1_args.push(format!("position = {input}${c}"));
        }

        // Build dataset2 R list.
        let d2 = &cfg.dataset2;
        let mut d2_args = vec![
            format!("snp = as.character({input}${})", r_str(&d2.snp)),
            format!("type = {}", r_str(&d2.r#type)),
        ];
        if let Some(c) = &d2.beta {
            d2_args.push(format!("beta = {input}${c}"));
        }
        if let Some(c) = &d2.varbeta {
            d2_args.push(format!("varbeta = {input}${c}"));
        }
        if let Some(c) = &d2.pvalues {
            d2_args.push(format!("pvalues = {input}${c}"));
        }
        if let Some(c) = &d2.maf {
            d2_args.push(format!("MAF = {input}${c}"));
        }
        if let Some(n) = d2.n {
            d2_args.push(format!("N = {n}"));
        }
        if let Some(s) = d2.s {
            d2_args.push(format!("s = {s}"));
        }
        if let Some(sdy) = d2.sd_y {
            d2_args.push(format!("sdY = {sdy}"));
        }
        if let Some(c) = &d2.position {
            d2_args.push(format!("position = {input}${c}"));
        }

        // Build coloc.abf call arguments.
        let mut coloc_args = vec![
            "dataset1 = dataset1".to_string(),
            "dataset2 = dataset2".to_string(),
            format!("p1 = {}", cfg.p1),
            format!("p2 = {}", cfg.p2),
            format!("p12 = {}", cfg.p12),
        ];
        if let Some(maf_col) = &cfg.maf {
            coloc_args.push(format!("MAF = {input}${maf_col}"));
        }
        if let Some(pw1_col) = &cfg.prior_weights1 {
            coloc_args.push(format!("prior_weights1 = {input}${pw1_col}"));
        }
        if let Some(pw2_col) = &cfg.prior_weights2 {
            coloc_args.push(format!("prior_weights2 = {input}${pw2_col}"));
        }

        let code = vec![
            "# coloc.abf: Bayesian colocalisation analysis".to_string(),
            "library(coloc)".to_string(),
            format!("dataset1 <- list({})", d1_args.join(", ")),
            format!("dataset2 <- list({})", d2_args.join(", ")),
            format!("{out} <- coloc.abf({})", coloc_args.join(", ")),
            format!("print({out}$summary)"),
        ];

        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["coloc".into()]
    }
}

#[async_trait]
impl DagNode for ColocAbfNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        COLOC_ABF_NODE_KIND
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
        let input = inputs.first().ok_or(ColocNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: COLOC_ABF_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(ColocNodeError::EmptyInput.into());
        }

        let cfg = &self.config;
        let d1 = build_dataset(&batches, &cfg.dataset1, cfg.maf.as_deref())?;
        let d2 = build_dataset(&batches, &cfg.dataset2, cfg.maf.as_deref())?;

        // Prior weights.
        let pw1 = match &cfg.prior_weights1 {
            Some(col) => Some(extract_f64(&batches, col)?),
            None => None,
        };
        let pw2 = match &cfg.prior_weights2 {
            Some(col) => Some(extract_f64(&batches, col)?),
            None => None,
        };

        let opts = coloc::ColocAbfOptions {
            p1: cfg.p1,
            p2: cfg.p2,
            p12: cfg.p12,
            prior_weights1: pw1,
            prior_weights2: pw2,
            maf: None,
        };

        let result = coloc::coloc_abf(&d1, &d2, &opts).map_err(ColocNodeError::Coloc)?;

        let batch = build_result_batch(&result)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(ColocNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build a coloc `Dataset` from the spec + DataFrame columns.
fn build_dataset(
    batches: &[RecordBatch],
    spec: &DatasetSpec,
    common_maf: Option<&str>,
) -> Result<coloc::Dataset, ColocNodeError> {
    let snp = extract_string(batches, &spec.snp)?;
    let r#type: coloc::TraitType =
        spec.r#type
            .as_str()
            .parse()
            .map_err(|_| ColocNodeError::WrongColumnType {
                name: "type".into(),
                dtype: spec.r#type.clone(),
            })?;

    let beta = spec
        .beta
        .as_ref()
        .and_then(|c| extract_opt_f64(batches, c).ok().flatten());
    let varbeta = spec
        .varbeta
        .as_ref()
        .and_then(|c| extract_opt_f64(batches, c).ok().flatten());
    let pvalues = spec
        .pvalues
        .as_ref()
        .and_then(|c| extract_opt_f64(batches, c).ok().flatten());

    // MAF: use dataset-specific column, or common MAF column.
    let maf_col = spec.maf.as_deref().or(common_maf);
    let maf = maf_col.and_then(|c| extract_opt_f64(batches, c).ok().flatten());

    let position = spec
        .position
        .as_ref()
        .and_then(|c| extract_opt_f64(batches, c).ok().flatten());

    Ok(coloc::Dataset {
        snp,
        beta,
        varbeta,
        pvalues,
        maf,
        n: spec.n,
        r#type,
        s: spec.s,
        sd_y: spec.sd_y,
        position,
    })
}

/// Build the long-format output `RecordBatch` from the coloc result.
fn build_result_batch(result: &coloc::ColocAbfResult) -> Result<RecordBatch, ColocNodeError> {
    let mut section: Vec<&str> = Vec::new();
    let mut nsnps: Vec<Option<i64>> = Vec::new();
    let mut pp_h0: Vec<Option<f64>> = Vec::new();
    let mut pp_h1: Vec<Option<f64>> = Vec::new();
    let mut pp_h2: Vec<Option<f64>> = Vec::new();
    let mut pp_h3: Vec<Option<f64>> = Vec::new();
    let mut pp_h4: Vec<Option<f64>> = Vec::new();
    let mut snp: Vec<Option<String>> = Vec::new();
    let mut l_abf_df1: Vec<Option<f64>> = Vec::new();
    let mut l_abf_df2: Vec<Option<f64>> = Vec::new();
    let mut snp_pp_h4: Vec<Option<f64>> = Vec::new();

    // Summary row.
    section.push("summary");
    nsnps.push(Some(result.nsnps as i64));
    pp_h0.push(Some(result.pp.pp_h0));
    pp_h1.push(Some(result.pp.pp_h1));
    pp_h2.push(Some(result.pp.pp_h2));
    pp_h3.push(Some(result.pp.pp_h3));
    pp_h4.push(Some(result.pp.pp_h4));
    snp.push(None);
    l_abf_df1.push(None);
    l_abf_df2.push(None);
    snp_pp_h4.push(None);

    // Per-SNP rows.
    for i in 0..result.nsnps {
        section.push("snp");
        nsnps.push(None);
        pp_h0.push(None);
        pp_h1.push(None);
        pp_h2.push(None);
        pp_h3.push(None);
        pp_h4.push(None);
        snp.push(Some(result.snp[i].clone()));
        l_abf_df1.push(Some(result.l_abf_df1[i]));
        l_abf_df2.push(Some(result.l_abf_df2[i]));
        snp_pp_h4.push(Some(result.snp_pp_h4[i]));
    }

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(section)),
            Arc::new(Int64Array::from(nsnps)),
            Arc::new(Float64Array::from(pp_h0)),
            Arc::new(Float64Array::from(pp_h1)),
            Arc::new(Float64Array::from(pp_h2)),
            Arc::new(Float64Array::from(pp_h3)),
            Arc::new(Float64Array::from(pp_h4)),
            Arc::new(StringArray::from(snp)),
            Arc::new(Float64Array::from(l_abf_df1)),
            Arc::new(Float64Array::from(l_abf_df2)),
            Arc::new(Float64Array::from(snp_pp_h4)),
        ],
    )?;
    Ok(batch)
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
        }
    }

    /// Build a small coloc input batch: 5 SNPs, two quantitative datasets.
    fn make_input_batch() -> RecordBatch {
        let snp = StringArray::from(vec!["s1", "s2", "s3", "s4", "s5"]);
        let beta1 = Float64Array::from(vec![0.5, 0.1, 0.3, -0.2, 0.05]);
        let vbeta1 = Float64Array::from(vec![0.01, 0.01, 0.01, 0.01, 0.01]);
        let beta2 = Float64Array::from(vec![0.4, 0.05, 0.25, -0.15, 0.02]);
        let vbeta2 = Float64Array::from(vec![0.01, 0.01, 0.01, 0.01, 0.01]);
        let maf = Float64Array::from(vec![0.3, 0.2, 0.4, 0.25, 0.35]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("snp", DataType::Utf8, false),
            Field::new("beta1", DataType::Float64, false),
            Field::new("varbeta1", DataType::Float64, false),
            Field::new("beta2", DataType::Float64, false),
            Field::new("varbeta2", DataType::Float64, false),
            Field::new("maf", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(snp),
                Arc::new(beta1),
                Arc::new(vbeta1),
                Arc::new(beta2),
                Arc::new(vbeta2),
                Arc::new(maf),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn runs_coloc_abf_node() {
        let mut node = ColocAbfNode::new(ColocAbfConfig {
            dataset1: DatasetSpec {
                r#type: "quant".into(),
                snp: "snp".into(),
                beta: Some("beta1".into()),
                varbeta: Some("varbeta1".into()),
                pvalues: None,
                maf: Some("maf".into()),
                n: Some(1000.0),
                s: None,
                sd_y: Some(1.0),
                position: None,
            },
            dataset2: DatasetSpec {
                r#type: "quant".into(),
                snp: "snp".into(),
                beta: Some("beta2".into()),
                varbeta: Some("varbeta2".into()),
                pvalues: None,
                maf: Some("maf".into()),
                n: Some(1000.0),
                s: None,
                sd_y: Some(1.0),
                position: None,
            },
            p1: 1e-4,
            p2: 1e-4,
            p12: 1e-5,
            maf: None,
            prior_weights1: None,
            prior_weights2: None,
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
        assert_eq!(batch.num_rows(), 6); // 1 summary + 5 SNPs

        // Summary row should have PP values.
        let section = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(section.value(0), "summary");
        let pp_h4 = batch
            .column(6)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let h4_val = pp_h4.value(0);
        assert!(h4_val > 0.0 && h4_val <= 1.0);
        // Similar effect directions → expect H4 dominant.
        assert!(h4_val > 0.5, "expected PP.H4 > 0.5, got {h4_val}");
    }
}
