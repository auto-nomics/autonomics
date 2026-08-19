//! Bivariate MiXeR (`fit2`) transform node — faithful port via gsa-mixer subprocess.
//!
//! 联合两个 GWAS trait 估计共享/特异 causal 变异与遗传相关。
//!
//! 输入（4 个端口）：
//! - port 0：trait1 sumstats（Z, N, rsid, A1, A2）
//! - port 1：trait2 sumstats（Z, N, rsid, A1, A2）
//! - port 2：trait1 的 univariate fit1 结果（pi, sig2_beta, sig2_zero）
//! - port 3：trait2 的 univariate fit1 结果（pi, sig2_beta, sig2_zero）
//!
//! 写临时文件，调用原版 `mixer.py fit2`（gsa-mixer v2.2.1 + libbgmg.so），
//! 解析 JSON 输出，返回单行 bivariate 结果。
//! 100% 数值保真——不在 Rust 中重新实现 cost function / optimizer。

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum BivariateMixerError {
    #[error("bivariate_mixer @ {context}: {detail}")]
    Step { context: String, detail: String },

    #[error("bivariate_mixer invalid input: {0}")]
    InvalidInput(String),

    #[error("bivariate_mixer arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("bivariate_mixer subprocess failed (exit code {exit_code}): {stderr}")]
    Subprocess { exit_code: i32, stderr: String },

    #[error("bivariate_mixer io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bivariate_mixer json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("bivariate_mixer reference bundle error: {0}")]
    ReferenceBundle(String),
}

impl ::dag_core::dag::NodeError for BivariateMixerError {
    fn node_type(&self) -> &str {
        "bivariate_mixer"
    }
}

// =====================================================================
// Schemas
// =====================================================================

const INPUT_RSID_COL: &str = "rsid";
const INPUT_A1_COL: &str = "A1";
const INPUT_A2_COL: &str = "A2";
const INPUT_Z_COL: &str = "Z";
const INPUT_N_COL: &str = "N";
const PARAM_PI: &str = "pi";
const PARAM_SB: &str = "sig2_beta";
const PARAM_SZ: &str = "sig2_zero";

fn sumstats_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
        Field::new(INPUT_A1_COL, DataType::Utf8, true),
        Field::new(INPUT_A2_COL, DataType::Utf8, true),
    ]))
}

fn fit1_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(PARAM_PI, DataType::Float64, false),
        Field::new(PARAM_SB, DataType::Float64, false),
        Field::new(PARAM_SZ, DataType::Float64, false),
    ]))
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pi1", DataType::Float64, false),
        Field::new("pi2", DataType::Float64, false),
        Field::new("pi12", DataType::Float64, false),
        Field::new("rho_beta", DataType::Float64, false),
        Field::new("rho_zero", DataType::Float64, false),
        Field::new("rg", DataType::Float64, false),
        Field::new("dice", DataType::Float64, false),
        Field::new("h2_t1", DataType::Float64, false),
        Field::new("h2_t2", DataType::Float64, false),
        Field::new("loglike", DataType::Float64, false),
    ]))
}

// =====================================================================
// Config / Spec
// =====================================================================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BivariateMixerNodeSpec {
    /// Deployed MiXeR reference bundle ID. Defaults to `g1000_eur`.
    #[serde(default = "crate::mixer_common::default_reference")]
    pub reference: String,

    /// 参与拟合的染色体范围。
    #[serde(default = "default_chr2use")]
    pub chr2use: String,

    /// 随机种子。
    #[serde(default = "default_seed")]
    pub seed: u64,

    /// 差分进化重复次数。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_fast_repeats: usize,

    /// 是否使用 fast-run 模式。
    #[serde(default = "default_fast_run")]
    pub fast_run: bool,

    /// kmax-pdf 参数。
    #[serde(default = "default_kmax_pdf")]
    pub kmax_pdf: u32,

    /// downsample-factor。
    #[serde(default = "default_downsample_factor")]
    pub downsample_factor: u32,
}

fn default_chr2use() -> String {
    "1-22".to_string()
}
fn default_seed() -> u64 {
    123
}
fn default_diffevo_repeats() -> usize {
    20
}
fn default_fast_run() -> bool {
    true
}
fn default_kmax_pdf() -> u32 {
    10
}
fn default_downsample_factor() -> u32 {
    1000
}

// =====================================================================
// Node
// =====================================================================

const BIVARIATE_MIXER_NODE_KIND: &str = "bivariate_mixer";

#[derive(Clone)]
pub struct BivariateMixerNode {
    meta: NodePorts,
    spec: BivariateMixerNodeSpec,
    reference_bundle: DataBundle,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(sumstats_schema()))
        .add_input_port(Some(sumstats_schema()))
        .add_input_port(Some(fit1_schema()))
        .add_input_port(Some(fit1_schema()))
        .add_output_port(Some(output_schema()))
}

impl BivariateMixerNode {
    pub fn new(spec: BivariateMixerNodeSpec, reference_bundle: DataBundle) -> Self {
        Self {
            meta: port_layout(),
            spec,
            reference_bundle,
        }
    }
}

pub struct BivariateMixerNodeFactory {}

impl NodeFactory for BivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        BIVARIATE_MIXER_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Fits bivariate MiXeR (fit2) via gsa-mixer subprocess."
    }
    fn doc(&self) -> &'static str {
        "Bivariate MiXeR (fit2) node — faithful port. Takes four inputs: \
        trait1 sumstats, trait2 sumstats, trait1 fit1 result, trait2 fit1 \
        result. Writes temp files, invokes `mixer.py fit2`, parses JSON. \
        One output port (pi1, pi2, pi12, rho_beta, rho_zero, rg, dice, h2_t1, h2_t2, loglike). \
        Sumstat column names are case-sensitive; use quoted SQL aliases for Z, N, A1, and A2."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BivariateMixerNodeSpec)
    }
    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let spec: BivariateMixerNodeSpec = serde_json::from_value(spec)?;
        Ok(vec![DataBundleBinding::new(
            "reference",
            format!("mixer.{}", spec.reference),
        )])
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: BivariateMixerNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(BivariateMixerNode::new(
            config,
            node_ctx.bound_data_bundle("reference")?.clone(),
        )))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<BivariateMixerNodeSpec>(spec, "bivariate_mixer")?;
        let reference = s.reference;
        let out = ctx.output_var.to_string();
        let code = vec![
            format!("# MiXeR bivariate analysis (gsa-mixer subprocess)"),
            "mixer_python <- Sys.getenv('MIXER_PYTHON', unset='<mixer_bundle_python>')".to_string(),
            format!("system2(mixer_python, c("),
            format!("  '<mixer_bundle:{reference}>/precimed/mixer.py>', 'fit2',"),
            "  '--lib', '<mixer_bundle_home>/libbgmg.so',".to_string(),
            format!("  '--out', '{out}'"),
            format!("))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

#[async_trait]
impl DagNode for BivariateMixerNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        BIVARIATE_MIXER_NODE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        use dag_core::dag::runtime::RuntimeStatus;
        let t0 = std::time::Instant::now();
        reporter.status(RuntimeStatus::Running);
        reporter.info(format!(
            "fit2 (gsa-mixer): start (chr2use={}, seed={}, fast_run={})",
            self.spec.chr2use, self.spec.seed, self.spec.fast_run,
        ));

        if inputs.len() != 4 {
            return Err(BivariateMixerError::InvalidInput(format!(
                "bivariate_mixer needs 4 inputs, got {}",
                inputs.len()
            ))
            .into());
        }

        // Find inputs by port index
        let sumstats1 = inputs
            .iter()
            .find(|i| i.port == 0)
            .ok_or_else(|| BivariateMixerError::InvalidInput("missing port 0".into()))?;
        let sumstats2 = inputs
            .iter()
            .find(|i| i.port == 1)
            .ok_or_else(|| BivariateMixerError::InvalidInput("missing port 1".into()))?;
        let fit1_t1 = inputs
            .iter()
            .find(|i| i.port == 2)
            .ok_or_else(|| BivariateMixerError::InvalidInput("missing port 2".into()))?;
        let fit1_t2 = inputs
            .iter()
            .find(|i| i.port == 3)
            .ok_or_else(|| BivariateMixerError::InvalidInput("missing port 3".into()))?;

        // ── 1. Validate sumstats columns ───────────────────────────────
        for (i, inp) in [sumstats1, sumstats2].iter().enumerate() {
            let sch = inp.dataframe()?.schema();
            for needed in [
                INPUT_Z_COL,
                INPUT_N_COL,
                INPUT_RSID_COL,
                INPUT_A1_COL,
                INPUT_A2_COL,
            ] {
                if !sch.fields().iter().any(|f| f.name() == needed) {
                    let avail: Vec<&str> = sch.fields().iter().map(|f| f.name().as_str()).collect();
                    return Err(BivariateMixerError::InvalidInput(format!(
                        "trait{} sumstats missing column '{needed}'; have: {avail:?}. \
                         MiXeR column names are case-sensitive; use quoted SQL aliases \
                         such as z AS \"Z\", n AS \"N\", a1 AS \"A1\", a2 AS \"A2\".",
                        i + 1
                    ))
                    .into());
                }
            }
        }

        // ── 2. Read fit1 constraints from port 2/3 ─────────────────────
        let (pi1, sb1, sz1) = read_fit1_constraint(fit1_t1.dataframe()?).await?;
        let (pi2, sb2, sz2) = read_fit1_constraint(fit1_t2.dataframe()?).await?;
        reporter.info(format!(
            "constraints: t1(pi={:.5}, sb={:.6}, sz={:.4})  t2(pi={:.5}, sb={:.6}, sz={:.4})",
            pi1, sb1, sz1, pi2, sb2, sz2
        ));

        // ── 3. Write temp files ────────────────────────────────────────
        let tmp_id = nanoid::nanoid!(8);
        let tmp_dir = std::env::temp_dir().join(format!("mixer_fit2_{tmp_id}"));
        std::fs::create_dir_all(&tmp_dir).map_err(BivariateMixerError::from)?;

        let ss1_path = tmp_dir.join("trait1.sumstats");
        let ss2_path = tmp_dir.join("trait2.sumstats");
        let params1_path = tmp_dir.join("trait1.fit.json");
        let params2_path = tmp_dir.join("trait2.fit.json");

        reporter.info("writing temp files...");
        write_sumstats(
            sumstats1.dataframe()?,
            &[
                (INPUT_RSID_COL, "SNP"),
                (INPUT_A1_COL, "A1"),
                (INPUT_A2_COL, "A2"),
                (INPUT_N_COL, "N"),
                (INPUT_Z_COL, "Z"),
            ],
            &ss1_path,
        )
        .await?;
        write_sumstats(
            sumstats2.dataframe()?,
            &[
                (INPUT_RSID_COL, "SNP"),
                (INPUT_A1_COL, "A1"),
                (INPUT_A2_COL, "A2"),
                (INPUT_N_COL, "N"),
                (INPUT_Z_COL, "Z"),
            ],
            &ss2_path,
        )
        .await?;

        // Write minimal fit1 JSON params files (mixer.py fit2 only reads params)
        write_minimal_fit1_json(pi1, sb1, sz1, &params1_path)?;
        write_minimal_fit1_json(pi2, sb2, sz2, &params2_path)?;

        // ── 4. Build mixer.py fit2 command ─────────────────────────────
        let bundle = crate::mixer_common::resolve_reference(
            node_ctx,
            &self.reference_bundle,
            &self.spec.reference,
        )
        .await
        .map_err(BivariateMixerError::ReferenceBundle)?;
        let mixer_py = bundle.mixer_home.join("precimed").join("mixer.py");
        let lib_path = bundle.mixer_home.join("libbgmg.so");
        let out_prefix = tmp_dir.join("result");

        let mut cmd =
            std::process::Command::new(crate::mixer_common::python_executable(&bundle.mixer_home));
        cmd.arg(&mixer_py)
            .arg("fit2")
            .arg("--bim-file")
            .arg(&bundle.bim_template)
            .arg("--ld-file")
            .arg(&bundle.ld_template)
            .arg("--lib")
            .arg(&lib_path)
            .arg("--extract")
            .arg(&bundle.extract_template)
            .arg("--trait1-file")
            .arg(&ss1_path)
            .arg("--trait2-file")
            .arg(&ss2_path)
            .arg("--trait1-params-file")
            .arg(&params1_path)
            .arg("--trait2-params-file")
            .arg(&params2_path)
            .arg("--chr2use")
            .arg(&self.spec.chr2use)
            .arg("--seed")
            .arg(self.spec.seed.to_string())
            .arg("--out")
            .arg(&out_prefix)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if self.spec.fast_run {
            cmd.arg("--fit-sequence")
                .arg("diffevo-fast")
                .arg("neldermead-fast")
                .arg("--diffevo-fast-repeats")
                .arg(self.spec.diffevo_fast_repeats.to_string());
        } else {
            cmd.arg("--fit-sequence").arg("diffevo").arg("neldermead");
        }
        cmd.arg("--kmax-pdf").arg(self.spec.kmax_pdf.to_string());
        cmd.arg("--downsample-factor")
            .arg(self.spec.downsample_factor.to_string());

        // ── 5. Run mixer.py fit2 ───────────────────────────────────────
        reporter.info("fit2: invoking gsa-mixer subprocess");
        let output = tokio::task::spawn_blocking(move || cmd.output())
            .await
            .map_err(|e| BivariateMixerError::Step {
                context: "subprocess join".into(),
                detail: e.to_string(),
            })?
            .map_err(BivariateMixerError::from)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lines: Vec<&str> = stderr.lines().collect();
            let start = lines.len().saturating_sub(20);
            let tail = lines[start..].join("\n");
            reporter.error(format!("fit2: gsa-mixer failed\n{tail}"));
            return Err(BivariateMixerError::Subprocess {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: tail,
            }
            .into());
        }

        reporter.info(format!(
            "fit2: gsa-mixer completed in {:.1}s",
            t0.elapsed().as_secs_f64()
        ));

        // ── 6. Parse result JSON ───────────────────────────────────────
        let json_path = format!("{}.json", out_prefix.display());
        let json_str =
            std::fs::read_to_string(&json_path).map_err(|e| BivariateMixerError::Step {
                context: format!("read result json ({})", json_path),
                detail: e.to_string(),
            })?;
        let json: serde_json::Value =
            serde_json::from_str(&json_str).map_err(BivariateMixerError::from)?;
        let result = parse_fit2_json(&json)?;

        reporter.info(format!(
            "fit2 result: pi1={:.2e} pi2={:.2e} pi12={:.2e} rho_beta={:.4} rho_zero={:.4} \
             rg={:.4} dice={:.4}",
            result.pi1,
            result.pi2,
            result.pi12,
            result.rho_beta,
            result.rho_zero,
            result.rg,
            result.dice,
        ));

        // ── 7. Build output RecordBatch ────────────────────────────────
        let batch = build_result_batch(&result)?;
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| BivariateMixerError::Step {
                context: "read result batch".into(),
                detail: e.to_string(),
            })?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        reporter.info(format!(
            "fit2: finished in {:.2}s",
            t0.elapsed().as_secs_f64()
        ));

        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(res)
    }
}

// =====================================================================
// Helpers: constraint reading, JSON writing, sumstats writing
// =====================================================================

/// Read (pi, sig2_beta, sig2_zero) from a fit1 result DataFrame.
async fn read_fit1_constraint(
    df: &datafusion::dataframe::DataFrame,
) -> Result<(f64, f64, f64), BivariateMixerError> {
    let batches = df
        .clone()
        .select_columns(&[PARAM_PI, PARAM_SB, PARAM_SZ])
        .map_err(|e| BivariateMixerError::Step {
            context: "select fit1 columns".into(),
            detail: e.to_string(),
        })?
        .collect()
        .await
        .map_err(|e| BivariateMixerError::Step {
            context: "collect fit1".into(),
            detail: e.to_string(),
        })?;

    let batch = batches.first().ok_or(BivariateMixerError::InvalidInput(
        "fit1 result is empty".into(),
    ))?;

    let pi = batch
        .column_by_name(PARAM_PI)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or(BivariateMixerError::InvalidInput(
            "missing pi column".into(),
        ))?
        .value(0);
    let sb = batch
        .column_by_name(PARAM_SB)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or(BivariateMixerError::InvalidInput(
            "missing sig2_beta column".into(),
        ))?
        .value(0);
    let sz = batch
        .column_by_name(PARAM_SZ)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or(BivariateMixerError::InvalidInput(
            "missing sig2_zero column".into(),
        ))?
        .value(0);

    Ok((pi, sb, sz))
}

/// Write a minimal fit1 JSON that mixer.py fit2 can consume.
/// mixer.py only reads `params.pi`, `params.sig2_beta`, `params.sig2_zero`.
fn write_minimal_fit1_json(
    pi: f64,
    sig2_beta: f64,
    sig2_zero: f64,
    path: &std::path::Path,
) -> Result<(), BivariateMixerError> {
    let json = serde_json::json!({
        "analysis": "univariate",
        "params": {
            "pi": pi,
            "sig2_beta": sig2_beta,
            "sig2_zero": sig2_zero
        }
    });
    std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

/// Write selected columns from a DataFrame to a plain TSV file.
async fn write_sumstats(
    df: &datafusion::dataframe::DataFrame,
    col_map: &[(&str, &str)],
    path: &std::path::Path,
) -> Result<(), BivariateMixerError> {
    use std::io::{BufWriter, Write};

    let batches = df
        .clone()
        .select(
            col_map
                .iter()
                .map(|(src, dst)| {
                    datafusion::prelude::cast(
                        datafusion::prelude::Expr::Column(datafusion::common::Column::from_name(
                            *src,
                        )),
                        DataType::Utf8,
                    )
                    .alias(*dst)
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| BivariateMixerError::Step {
            context: "select columns".into(),
            detail: e.to_string(),
        })?
        .collect()
        .await
        .map_err(|e| BivariateMixerError::Step {
            context: "collect batches".into(),
            detail: e.to_string(),
        })?;

    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::new(f);

    let header = col_map
        .iter()
        .map(|(_, dst)| *dst)
        .collect::<Vec<_>>()
        .join("\t");
    writeln!(w, "{header}")?;

    let mut n_rows = 0;
    for batch in &batches {
        let columns: Vec<&dyn arrow_array::Array> = col_map
            .iter()
            .map(|(_, dst)| {
                batch
                    .column_by_name(dst)
                    .expect("column should exist after select")
                    .as_ref()
            })
            .collect();

        for row in 0..batch.num_rows() {
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    write!(w, "\t")?;
                }
                if col.is_null(row) {
                    return Err(BivariateMixerError::Step {
                        context: "write sumstats".into(),
                        detail: format!(
                            "null in output column '{}' at batch row {row}",
                            col_map[i].1
                        ),
                    });
                }
                let val = arrow_array::cast::as_string_array(*col);
                write!(w, "{}", val.value(row))?;
            }
            writeln!(w)?;
            n_rows += 1;
        }
    }
    w.flush()?;
    info!("wrote {} sumstats rows to {}", n_rows, path.display());
    Ok(())
}

// =====================================================================
// JSON parsing
// =====================================================================

struct BivariateResult {
    pi1: f64,
    pi2: f64,
    pi12: f64,
    rho_beta: f64,
    rho_zero: f64,
    rg: f64,
    dice: f64,
    h2_t1: f64,
    h2_t2: f64,
    loglike: f64,
}

fn parse_fit2_json(json: &serde_json::Value) -> Result<BivariateResult, BivariateMixerError> {
    let p = json
        .get("params")
        .ok_or_else(|| BivariateMixerError::Step {
            context: "parse json".into(),
            detail: "missing 'params'".into(),
        })?;

    let pi_arr = p["pi"]
        .as_array()
        .ok_or_else(|| BivariateMixerError::Step {
            context: "parse json".into(),
            detail: "params.pi is not an array".into(),
        })?;
    let pi1 = pi_arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pi2 = pi_arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pi12 = pi_arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0);

    let rho_beta = p["rho_beta"].as_f64().unwrap_or(0.0);
    let rho_zero = p["rho_zero"].as_f64().unwrap_or(0.0);

    let ci_point = |key: &str| {
        json.get("ci")
            .and_then(|ci| ci.get(key))
            .and_then(|value| value.get("point_estimate"))
            .and_then(|value| value.as_f64())
    };

    let denom = pi1 + pi2 + 2.0 * pi12;
    let pi1 = ci_point("pi1").unwrap_or(pi1);
    let pi2 = ci_point("pi2").unwrap_or(pi2);
    let pi12 = ci_point("pi12").unwrap_or(pi12);
    let rho_beta = ci_point("rho_beta").unwrap_or(rho_beta);
    let rho_zero = ci_point("rho_zero").unwrap_or(rho_zero);
    let rg = ci_point("rg").unwrap_or(0.0);
    let dice =
        ci_point("dice").unwrap_or_else(|| if denom > 0.0 { 2.0 * pi12 / denom } else { 0.0 });

    // Keep the documented deterministic fallback for older/custom JSON output.
    let sig2_beta_arr = p["sig2_beta"].as_array();
    let sb1 = sig2_beta_arr
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let sb2 = sig2_beta_arr
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let totalhet = json
        .get("options")
        .and_then(|o| o.get("totalhet"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let h2_t1 = ci_point("h2_T1").unwrap_or(sb1 * (pi1 + pi12) * totalhet);
    let h2_t2 = ci_point("h2_T2").unwrap_or(sb2 * (pi2 + pi12) * totalhet);

    // loglike from last optimize step
    let optimize = json.get("optimize").and_then(|v| v.as_array());
    let loglike = optimize
        .and_then(|arr| arr.last())
        .and_then(|last| last.as_array())
        .and_then(|pair| pair.get(1))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("fun"))
        .and_then(|f| f.as_f64())
        .unwrap_or(f64::NAN);

    Ok(BivariateResult {
        pi1,
        pi2,
        pi12,
        rho_beta,
        rho_zero,
        rg,
        dice,
        h2_t1,
        h2_t2,
        loglike,
    })
}

fn build_result_batch(r: &BivariateResult) -> Result<RecordBatch, BivariateMixerError> {
    let schema = output_schema();
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.pi1])),
            Arc::new(Float64Array::from(vec![r.pi2])),
            Arc::new(Float64Array::from(vec![r.pi12])),
            Arc::new(Float64Array::from(vec![r.rho_beta])),
            Arc::new(Float64Array::from(vec![r.rho_zero])),
            Arc::new(Float64Array::from(vec![r.rg])),
            Arc::new(Float64Array::from(vec![r.dice])),
            Arc::new(Float64Array::from(vec![r.h2_t1])),
            Arc::new(Float64Array::from(vec![r.h2_t2])),
            Arc::new(Float64Array::from(vec![r.loglike])),
        ],
    )?)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
    use datafusion::prelude::CsvReadOptions;

    fn reference_bundle() -> DataBundle {
        DataBundle::new(
            "mixer.g1000_eur",
            "MiXeR reference",
            "/bundles/mixer/g1000_eur",
        )
    }

    #[test]
    fn node_type_is_stable() {
        assert_eq!(BIVARIATE_MIXER_NODE_KIND, "bivariate_mixer");
    }

    #[test]
    fn spec_defaults() {
        let spec: BivariateMixerNodeSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.reference, "g1000_eur");
        let error =
            serde_json::from_str::<BivariateMixerNodeSpec>(r#"{"bim_file":"x"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown field `bim_file`"));

        let node = BivariateMixerNode::new(spec, reference_bundle());
        assert_eq!(node.kind(), "bivariate_mixer");
        assert_eq!(node.ports().input_ports().len(), 4);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    #[tokio::test]
    async fn write_sumstats_renames_and_stringifies_columns() {
        let ctx = datafusion::prelude::SessionContext::new();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new(INPUT_RSID_COL, DataType::Utf8, false),
                Field::new(INPUT_A1_COL, DataType::Utf8, true),
                Field::new(INPUT_A2_COL, DataType::Utf8, true),
                Field::new(INPUT_N_COL, DataType::Float64, true),
                Field::new(INPUT_Z_COL, DataType::Float64, true),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(vec!["rs1"])),
                Arc::new(arrow_array::StringArray::from(vec!["A"])),
                Arc::new(arrow_array::StringArray::from(vec!["G"])),
                Arc::new(Float64Array::from(vec![1000.0])),
                Arc::new(Float64Array::from(vec![1.5])),
            ],
        )
        .unwrap();
        let df = ctx.read_batch(batch).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trait1.sumstats");

        write_sumstats(
            &df,
            &[
                (INPUT_RSID_COL, "SNP"),
                (INPUT_A1_COL, "A1"),
                (INPUT_A2_COL, "A2"),
                (INPUT_N_COL, "N"),
                (INPUT_Z_COL, "Z"),
            ],
            &path,
        )
        .await
        .unwrap();

        let output = std::fs::read_to_string(&path).unwrap();
        assert_eq!(output, "SNP\tA1\tA2\tN\tZ\nrs1\tA\tG\t1000.0\t1.5\n");
    }

    #[test]
    fn parse_typical_fit2_json() {
        let json_str = r#"{
            "params": {
                "pi": [0.001, 0.005, 0.0003],
                "sig2_beta": [0.04, 0.006],
                "rho_beta": 0.85,
                "rho_zero": 0.24
            },
            "optimize": [["diffevo-fast", {"fun": 6826.5}]],
            "options": {"totalhet": 50000.0}
        }"#;
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let r = parse_fit2_json(&json).unwrap();
        assert!((r.pi1 - 0.001).abs() < 1e-10);
        assert!((r.pi12 - 0.0003).abs() < 1e-10);
        assert!((r.rho_beta - 0.85).abs() < 1e-6);
        assert!((r.dice - 2.0 * 0.0003 / (0.001 + 0.005 + 2.0 * 0.0003)).abs() < 1e-6);
    }

    #[tokio::test]
    async fn fit2_runs_against_deployed_reference_bundle() {
        let fixture_root = std::path::Path::new("/mnt/disk3/gsa-mixer/precimed/mixer-test/data");
        let engine_root = std::path::Path::new("/data/mixer/resources/g1000_eur");
        if !fixture_root.join("trait1.sumstats.gz").is_file()
            || !fixture_root.join("trait2.sumstats.gz").is_file()
            || !engine_root.join("bundle.json").is_file()
        {
            return;
        }
        let (node_ctx, _vfs_scratch) = crate::mixer_common::tests::vfs_ctx(engine_root);

        let ctx = datafusion::prelude::SessionContext::new();
        let options = || {
            CsvReadOptions::new()
                .has_header(true)
                .delimiter(b'\t')
                .file_extension("sumstats.gz")
                .file_compression_type(FileCompressionType::GZIP)
        };
        let trait1 = ctx
            .read_csv(
                fixture_root.join("trait1.sumstats.gz").to_str().unwrap(),
                options(),
            )
            .await
            .unwrap();
        let trait2 = ctx
            .read_csv(
                fixture_root.join("trait2.sumstats.gz").to_str().unwrap(),
                options(),
            )
            .await
            .unwrap();
        ctx.register_table("trait1_input", trait1.into_view())
            .unwrap();
        let trait1 = ctx
            .sql(r#"SELECT CONCAT("CHR", ':', "BP", ':', "A1", ':', "A2") AS "rsid", "A1", "A2", "N", "Z" FROM trait1_input"#)
            .await
            .unwrap();
        ctx.register_table("trait2_input", trait2.into_view())
            .unwrap();
        let trait2 = ctx
            .sql(r#"SELECT CONCAT("CHR", ':', "BP", ':', "A1", ':', "A2") AS "rsid", "A1", "A2", "N", "Z" FROM trait2_input"#)
            .await
            .unwrap();

        let fit1 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new(PARAM_PI, DataType::Float64, false),
                Field::new(PARAM_SB, DataType::Float64, false),
                Field::new(PARAM_SZ, DataType::Float64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![0.000786])),
                Arc::new(Float64Array::from(vec![0.08354])),
                Arc::new(Float64Array::from(vec![1.07528])),
            ],
        )
        .unwrap();
        let fit1_1 = ctx.read_batch(fit1.clone()).unwrap();
        let fit1_2 = ctx.read_batch(fit1).unwrap();

        let mut node = BivariateMixerNode::new(
            BivariateMixerNodeSpec {
                reference: "g1000_eur".into(),
                chr2use: "21-22".into(),
                seed: 123,
                diffevo_fast_repeats: 2,
                fast_run: true,
                kmax_pdf: 10,
                downsample_factor: 1000,
            },
            reference_bundle(),
        );
        let outputs = node
            .execute(
                &node_ctx,
                &[
                    NodeInput::new_dataframe(0, trait1),
                    NodeInput::new_dataframe(1, trait2),
                    NodeInput::new_dataframe(2, fit1_1),
                    NodeInput::new_dataframe(3, fit1_2),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            1
        );
    }
}
