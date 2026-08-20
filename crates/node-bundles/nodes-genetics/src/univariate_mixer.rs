//! Univariate MiXeR (`fit1`) transform node — Rust-native fitting.
//!
//! 接收上游 GWAS 汇总统计 `DataFrame`（含 rsid, A1, A2, N, Z），写临时文件，
//! 调用 Rust `mixer::fit1`，返回单行结果 `DataFrame`。
//!
//! `libbgmg.so` 仅作为二进制参考数据加载器使用：它负责原版 `.ld` 解码、sumstats
//! 等位基因对齐和 randprune。随后本节点把 tag 行折叠成充分统计量，释放 FFI
//! context，并用纯 Rust cost/optimizer 完成 `diffevo-fast -> neldermead-fast`。
//!
//! 返回单行结果 `DataFrame`（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）。

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::{Mutex, OnceLock};

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};
use libloading::{Library, Symbol};
use mixer::data::UnivariateSufficient;

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum UnivariateMixerError {
    #[error("univariate_mixer @ {context}: {detail}")]
    Step { context: String, detail: String },

    #[error("univariate_mixer invalid input: {0}")]
    InvalidInput(String),

    #[error("univariate_mixer arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("univariate_mixer FFI error @ {step}: {detail}")]
    Ffi { step: String, detail: String },

    #[error("univariate_mixer io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("univariate_mixer native fit error: {0}")]
    NativeFit(String),

    #[error("univariate_mixer reference bundle error: {0}")]
    ReferenceBundle(String),
}

impl ::dag_core::dag::NodeError for UnivariateMixerError {
    fn node_type(&self) -> &str {
        "univariate_mixer"
    }
}

// =====================================================================
// Schemas — unchanged from original, preserves downstream compatibility
// =====================================================================

const INPUT_RSID_COL: &str = "rsid";
const INPUT_A1_COL: &str = "A1";
const INPUT_A2_COL: &str = "A2";
const INPUT_Z_COL: &str = "Z";
const INPUT_N_COL: &str = "N";

/// 输入端口：需要 rsid, A1, A2, N, Z 五列（比原版多了 A1/A2，
/// 因为 bgmg 需要等位基因来做与参考面板的对齐）。
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
        Field::new(INPUT_A1_COL, DataType::Utf8, true),
        Field::new(INPUT_A2_COL, DataType::Utf8, true),
    ]))
}

/// 输出端口：与原版完全一致。
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pi", DataType::Float64, false),
        Field::new("sig2_beta", DataType::Float64, false),
        Field::new("sig2_zero", DataType::Float64, false),
        Field::new("h2", DataType::Float64, false),
        Field::new("nc", DataType::Float64, false),
        Field::new("nc_p9", DataType::Float64, false),
        Field::new("aic", DataType::Float64, false),
        Field::new("bic", DataType::Float64, false),
        Field::new("loglike", DataType::Float64, false),
    ]))
}

// =====================================================================
// Config / Spec
// =====================================================================

/// Univariate MiXeR 节点配置（DAG spec）。
///
/// Rust 原生 fit1 配置；路径由 reference bundle 绑定解析。
/// 所有路径参数指向 `reference/mixer_data/` 下的预计算文件。
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnivariateMixerNodeSpec {
    /// Deployed MiXeR reference bundle ID. Defaults to `g1000_eur`.
    #[serde(default = "crate::mixer_common::default_reference")]
    pub reference: String,

    /// 参与拟合的染色体范围，传给 `--chr2use`。
    /// 典型值：`"1-22"` 或 `"21-22"`
    #[serde(default = "default_chr2use")]
    pub chr2use: String,

    /// 随机种子。
    #[serde(default = "default_seed")]
    pub seed: u64,

    /// 差分进化重复次数（`--diffevo-fast-repeats`）。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_fast_repeats: usize,

    /// 是否使用 fast-run 模式（diffevo-fast + neldermead-fast）。
    /// Rust 原生实现当前只实现 Gaussian fast cost；false 会返回明确错误。
    #[serde(default = "default_fast_run")]
    pub fast_run: bool,

    /// kmax-pdf 参数（采样 cost 的 MC 实现数；越大越精确但越慢）。
    #[serde(default = "default_kmax_pdf")]
    pub kmax_pdf: u32,

    /// downsample-factor（跳过部分 tag 以加速；1000=快速，1=精确）。
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

fn parse_chr2use(value: &str) -> Result<Vec<i32>, UnivariateMixerError> {
    let mut chromosomes = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let invalid = || UnivariateMixerError::InvalidInput(format!("invalid chr2use '{value}'"));
        let bounds: Vec<&str> = part.splitn(2, '-').collect();
        let (start, end) = if bounds.len() == 2 {
            let start: i32 = bounds[0].trim().parse().map_err(|_| invalid())?;
            let end: i32 = bounds[1].trim().parse().map_err(|_| invalid())?;
            (start, end)
        } else {
            let chromosome: i32 = part.parse().map_err(|_| invalid())?;
            (chromosome, chromosome)
        };
        if start <= 0 || end < start {
            return Err(UnivariateMixerError::InvalidInput(format!(
                "invalid chr2use range '{part}'"
            )));
        }
        chromosomes.extend(start..=end);
    }
    if chromosomes.is_empty() {
        return Err(UnivariateMixerError::InvalidInput(
            "chr2use cannot be empty".into(),
        ));
    }
    Ok(chromosomes)
}

// =====================================================================
// Node
// =====================================================================

const UNIVARIATE_MIXER_NODE_KIND: &str = "univariate_mixer";

#[derive(Clone)]
pub struct UnivariateMixerNode {
    meta: NodePorts,
    spec: UnivariateMixerNodeSpec,
    reference_bundle: DataBundle,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl UnivariateMixerNode {
    pub fn new(spec: UnivariateMixerNodeSpec, reference_bundle: DataBundle) -> Self {
        Self {
            meta: port_layout(),
            spec,
            reference_bundle,
        }
    }
}

pub struct UnivariateMixerNodeFactory {}

impl NodeFactory for UnivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Fits univariate MiXeR spike-and-slab (fit1) with the Rust-native mixer engine."
    }

    fn doc(&self) -> &'static str {
        "Univariate MiXeR (fit1) node — Rust-native fit. Writes upstream GWAS \
        summary statistics to a temp file, uses libbgmg.so only to load and align \
        the reference bundle, folds LD into sufficient statistics, and runs the \
        Rust optimizer. This is validated against gsa-MiXeR but is not bit-exact. \
        One typed input port (rsid, A1, A2, N, Z); one typed output port. \
        Column names are case-sensitive. DataFusion SQL lowercases unquoted \
        aliases, so use quoted aliases such as AS \"Z\" and AS \"A1\"."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UnivariateMixerNodeSpec)
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let spec: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
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
        let config: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let node =
            UnivariateMixerNode::new(config, node_ctx.bound_data_bundle("reference")?.clone());
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<UnivariateMixerNodeSpec>(spec, "univariate_mixer")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let code = vec![
            "# MiXeR univariate analysis (reference gsa-mixer engine)".to_string(),
            "tmp_sumstats <- tempfile(fileext = '.sumstats.gz')".to_string(),
            format!("data.table::fwrite({input}, tmp_sumstats, sep = '\\t')"),
            "mixer_python <- Sys.getenv('MIXER_PYTHON', unset='<mixer_bundle_python>')".to_string(),
            "system2(mixer_python, c(".to_string(),
            "  '<mixer_bundle_home>/precimed/mixer.py', 'fit1',".to_string(),
            format!(
                "  '--bim-file', '<mixer_bundle:{}>/bim_template>',",
                s.reference
            ),
            "  '--ld-file', '<mixer_bundle:ld_template>',".to_string(),
            "  '--lib', '<mixer_bundle_home>/libbgmg.so',".to_string(),
            "  '--extract', '<mixer_bundle:extract_template>',".to_string(),
            "  '--trait1-file', tmp_sumstats,".to_string(),
            format!("  '--chr2use', '{}',", s.chr2use),
            format!("  '--seed', '{}',", s.seed),
            format!("  '--out', '{out}'"),
            "))".to_string(),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

#[async_trait]
impl DagNode for UnivariateMixerNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
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
            "fit1 (Rust-native): start (chr2use={}, seed={}, fast_run={})",
            self.spec.chr2use, self.spec.seed, self.spec.fast_run,
        ));
        let chromosomes = parse_chr2use(&self.spec.chr2use)?;
        if !self.spec.fast_run {
            return Err(UnivariateMixerError::NativeFit(
                "fast_run=false requires the convolve cost calculator, which is not implemented in the Rust fit1 path".into(),
            )
            .into());
        }

        let input = inputs.first().ok_or(UnivariateMixerError::InvalidInput(
            "no input DataFrame".into(),
        ))?;

        // ── 1. Validate input columns ──────────────────────────────────
        let schema = input.dataframe()?.schema();
        let missing: Vec<&str> = [
            INPUT_Z_COL,
            INPUT_N_COL,
            INPUT_RSID_COL,
            INPUT_A1_COL,
            INPUT_A2_COL,
        ]
        .into_iter()
        .filter(|needed| !schema.fields().iter().any(|f| f.name() == *needed))
        .collect();

        if !missing.is_empty() {
            let avail: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            return Err(UnivariateMixerError::InvalidInput(format!(
                "upstream sumstats missing required columns {missing:?}; have: {avail:?}. \
                 MiXeR column names are case-sensitive; in a preceding SQL node use \
                 quoted aliases such as z AS \"Z\", n AS \"N\", a1 AS \"A1\", a2 AS \"A2\"."
            ))
            .into());
        }

        // ── 2. Write sumstats to temp file ─────────────────────────────
        // bgmg_init's battle-tested parser handles rsid/A1/A2 alignment. Keeping
        // this boundary avoids re-implementing ambiguous allele and flip rules.
        let tmp_dir = tempfile::tempdir().map_err(UnivariateMixerError::from)?;
        let sumstats_path = tmp_dir.path().join("trait1.sumstats");

        reporter.info("writing sumstats to temp file...");
        write_sumstats(
            input.dataframe()?,
            &[
                (INPUT_RSID_COL, "SNP"),
                (INPUT_A1_COL, "A1"),
                (INPUT_A2_COL, "A2"),
                (INPUT_N_COL, "N"),
                (INPUT_Z_COL, "Z"),
            ],
            &sumstats_path,
        )
        .await?;

        let n_snp = count_lines(&sumstats_path).saturating_sub(1); // minus header
        reporter.info(format!("wrote {n_snp} SNPs to {}", sumstats_path.display()));

        // ── 3. Load reference data and fit in Rust ─────────────────────
        let bundle = crate::mixer_common::resolve_reference(
            node_ctx,
            &self.reference_bundle,
            &self.spec.reference,
        )
        .await
        .map_err(UnivariateMixerError::ReferenceBundle)?;
        let lib_path = bundle.mixer_home.join("libbgmg.so");

        reporter
            .info("fit1: loading reference through libbgmg FFI, then fitting with Rust optimizer");
        let spec = self.spec.clone();
        let native = tokio::task::spawn_blocking(move || {
            load_and_fit_native(
                &lib_path,
                &bundle.bim_template,
                &bundle.ld_template,
                &bundle.extract_template,
                &sumstats_path,
                &chromosomes,
                &spec,
            )
        })
        .await
        .map_err(|e| UnivariateMixerError::Step {
            context: "native fit join".into(),
            detail: e.to_string(),
        })??;

        reporter.info(format!(
            "fit1: loaded {} reference SNPs and {} tags (sum weights {:.2}); Rust fit complete",
            native.num_snp, native.num_tag, native.sum_weights,
        ));
        let result = native.result;
        reporter.info(format!(
            "fit1 result: pi={:.4} sig2_beta={:.4} sig2_zero={:.4} h2={:.4} \
             nc={:.0} nc_p9={:.0} loglike={:.2} aic={:.2} bic={:.2}",
            result.params.pi,
            result.params.sig2_beta,
            result.params.sig2_zero,
            result.h2,
            result.nc,
            result.nc_p9,
            result.loglike,
            result.aic,
            result.bic,
        ));

        // Flag degeneracies (same warnings as original)
        if result.params.pi > 0.999 || result.params.pi < 1e-4 {
            reporter.warn(format!(
                "fit1: pi={:.4} at boundary — spike-and-slab identifiability degeneracy",
                result.params.pi
            ));
        }
        if !result.loglike.is_finite() || !result.aic.is_finite() || !result.bic.is_finite() {
            reporter.warn(format!(
                "fit1: non-finite goodness-of-fit (loglike={:?} aic={:?} bic={:?})",
                result.loglike, result.aic, result.bic
            ));
        }

        // ── 6. Build output RecordBatch ────────────────────────────────
        let batch = build_result_batch(&result)?;
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| UnivariateMixerError::Step {
                context: "read result batch into DataFrame".into(),
                detail: e.to_string(),
            })?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        reporter.info(format!(
            "fit1: finished in {:.2}s",
            t0.elapsed().as_secs_f64()
        ));

        Ok(res)
    }
}

struct NativeFitOutput {
    result: mixer::result::FitResult,
    num_snp: usize,
    num_tag: usize,
    sum_weights: f64,
}

fn load_and_fit_native(
    library_path: &Path,
    bim_template: &str,
    ld_template: &str,
    extract_template: &str,
    sumstats_path: &Path,
    chromosomes: &[i32],
    spec: &UnivariateMixerNodeSpec,
) -> Result<NativeFitOutput, UnivariateMixerError> {
    static LIBBGMG_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LIBBGMG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| UnivariateMixerError::Ffi {
            step: "lock".into(),
            detail: "libbgmg lock poisoned".into(),
        })?;

    let sufficient = unsafe {
        load_sufficient_through_libbgmg(
            library_path,
            bim_template,
            ld_template,
            extract_template,
            sumstats_path,
            chromosomes,
            spec.seed,
        )?
    };
    let num_snp = sufficient.n_snp;
    let num_tag = sufficient.n_tag();
    let sum_weights = sufficient.weights.iter().sum();

    let config = mixer::fit::FitConfig {
        diffevo_repeats: spec.diffevo_fast_repeats,
        seed: spec.seed,
        ..mixer::fit::FitConfig::default()
    };
    let result = mixer::fit::fit1(&sufficient, &config);
    Ok(NativeFitOutput {
        result,
        num_snp,
        num_tag,
        sum_weights,
    })
}

macro_rules! load_symbol {
    ($library:expr, $name:expr, $ty:ty, $step:expr) => {{
        unsafe { $library.get::<$ty>($name) }.map_err(|error| UnivariateMixerError::Ffi {
            step: $step.into(),
            detail: error.to_string(),
        })
    }};
}

unsafe fn last_error(library: &Library) -> String {
    let error: Symbol<unsafe extern "C" fn() -> *const c_char> = load_symbol!(
        library,
        b"bgmg_get_last_error\0",
        unsafe extern "C" fn() -> *const c_char,
        "load bgmg_get_last_error"
    )
    .unwrap();
    unsafe { CStr::from_ptr(error()) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn check(
    library: &Library,
    code: i64,
    step: &'static str,
) -> Result<(), UnivariateMixerError> {
    if code == 0 {
        Ok(())
    } else {
        Err(UnivariateMixerError::Ffi {
            step: step.into(),
            detail: format!("libbgmg returned {code}: {}", unsafe {
                last_error(library)
            }),
        })
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn load_sufficient_through_libbgmg(
    library_path: &Path,
    bim_template: &str,
    ld_template: &str,
    extract_template: &str,
    sumstats_path: &Path,
    chromosomes: &[i32],
    seed: u64,
) -> Result<UnivariateSufficient, UnivariateMixerError> {
    static NEXT_CONTEXT: AtomicI32 = AtomicI32::new(1);
    let context_id = NEXT_CONTEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let library = Library::new(library_path).map_err(|error| UnivariateMixerError::Ffi {
        step: "load libbgmg.so".into(),
        detail: format!("{}: {error}", library_path.display()),
    })?;

    let result = (|| {
        type InitFn = unsafe extern "C" fn(
            c_int,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
        ) -> i64;
        type SetOptionFn = unsafe extern "C" fn(c_int, *mut c_char, c_double) -> i64;
        type SetLdFileFn = unsafe extern "C" fn(c_int, c_int, *const c_char) -> i64;
        type SetLdCsrFn = unsafe extern "C" fn(c_int, c_int) -> i64;
        type SetRandpruneFn = unsafe extern "C" fn(
            c_int,
            c_int,
            f32,
            f32,
            c_int,
            *const c_char,
            *const c_char,
        ) -> i64;
        type GetCountFn = unsafe extern "C" fn(c_int) -> i64;
        type RetrieveTagIndicesFn = unsafe extern "C" fn(c_int, c_int, *mut i32) -> i64;
        type RetrieveF32VecFn = unsafe extern "C" fn(c_int, c_int, *mut f32) -> i64;
        type RetrieveTraitVecFn = unsafe extern "C" fn(c_int, c_int, c_int, *mut f32) -> i64;
        type NumLdTagFn = unsafe extern "C" fn(c_int, c_int) -> i64;
        type RetrieveLdTagFn = unsafe extern "C" fn(c_int, c_int, c_int, *mut i32, *mut f32) -> i64;
        type InitLogFn = unsafe extern "C" fn(*const c_char);

        let init_log: Symbol<InitLogFn> = load_symbol!(
            &library,
            b"bgmg_init_log\0",
            InitLogFn,
            "load bgmg_init_log"
        )?;
        let init: Symbol<InitFn> =
            load_symbol!(&library, b"bgmg_init\0", InitFn, "load bgmg_init")?;
        let set_option: Symbol<SetOptionFn> = load_symbol!(
            &library,
            b"bgmg_set_option\0",
            SetOptionFn,
            "load bgmg_set_option"
        )?;
        let set_ld_file: Symbol<SetLdFileFn> = load_symbol!(
            &library,
            b"bgmg_set_ld_r2_coo_from_file\0",
            SetLdFileFn,
            "load bgmg_set_ld_r2_coo_from_file"
        )?;
        let set_ld_csr: Symbol<SetLdCsrFn> = load_symbol!(
            &library,
            b"bgmg_set_ld_r2_csr\0",
            SetLdCsrFn,
            "load bgmg_set_ld_r2_csr"
        )?;
        let set_randprune: Symbol<SetRandpruneFn> = load_symbol!(
            &library,
            b"bgmg_set_weights_randprune\0",
            SetRandpruneFn,
            "load bgmg_set_weights_randprune"
        )?;
        let get_num_tag: Symbol<GetCountFn> = load_symbol!(
            &library,
            b"bgmg_get_num_tag\0",
            GetCountFn,
            "load bgmg_get_num_tag"
        )?;
        let get_num_snp: Symbol<GetCountFn> = load_symbol!(
            &library,
            b"bgmg_get_num_snp\0",
            GetCountFn,
            "load bgmg_get_num_snp"
        )?;
        let retrieve_tag_indices: Symbol<RetrieveTagIndicesFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_tag_indices\0",
            RetrieveTagIndicesFn,
            "load bgmg_retrieve_tag_indices"
        )?;
        let retrieve_mafvec: Symbol<RetrieveF32VecFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_mafvec\0",
            RetrieveF32VecFn,
            "load bgmg_retrieve_mafvec"
        )?;
        let retrieve_zvec: Symbol<RetrieveTraitVecFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_zvec\0",
            RetrieveTraitVecFn,
            "load bgmg_retrieve_zvec"
        )?;
        let retrieve_nvec: Symbol<RetrieveTraitVecFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_nvec\0",
            RetrieveTraitVecFn,
            "load bgmg_retrieve_nvec"
        )?;
        let retrieve_weights: Symbol<RetrieveF32VecFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_weights\0",
            RetrieveF32VecFn,
            "load bgmg_retrieve_weights"
        )?;
        let num_ld_tag: Symbol<NumLdTagFn> = load_symbol!(
            &library,
            b"bgmg_num_ld_r_tag\0",
            NumLdTagFn,
            "load bgmg_num_ld_r_tag"
        )?;
        let retrieve_ld_tag: Symbol<RetrieveLdTagFn> = load_symbol!(
            &library,
            b"bgmg_retrieve_ld_r_tag\0",
            RetrieveLdTagFn,
            "load bgmg_retrieve_ld_r_tag"
        )?;

        let log_path = sumstats_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("libbgmg.log");
        let log_path = CString::new(log_path.to_string_lossy().as_bytes()).map_err(|error| {
            UnivariateMixerError::Ffi {
                step: "create libbgmg log path".into(),
                detail: error.to_string(),
            }
        })?;
        init_log(log_path.as_ptr());

        let empty = CString::new("").map_err(|error| UnivariateMixerError::Ffi {
            step: "create empty FFI string".into(),
            detail: error.to_string(),
        })?;
        let bim = CString::new(bim_template).map_err(|error| UnivariateMixerError::Ffi {
            step: "create bim path".into(),
            detail: error.to_string(),
        })?;
        let trait1 = CString::new(sumstats_path.to_string_lossy().as_bytes()).map_err(|error| {
            UnivariateMixerError::Ffi {
                step: "create trait path".into(),
                detail: error.to_string(),
            }
        })?;
        let chr_labels = CString::new(
            chromosomes
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
        .map_err(|error| UnivariateMixerError::Ffi {
            step: "create chr labels".into(),
            detail: error.to_string(),
        })?;
        let combined_extract_path = sumstats_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("extract.snps");
        {
            use std::io::{BufWriter, Write};

            let output = std::fs::File::create(&combined_extract_path).map_err(|error| {
                UnivariateMixerError::Ffi {
                    step: "create combined extract".into(),
                    detail: format!("{}: {error}", combined_extract_path.display()),
                }
            })?;
            let mut output = BufWriter::new(output);
            for chromosome in chromosomes {
                let source = extract_template.replace('@', &chromosome.to_string());
                std::io::copy(
                    &mut std::fs::File::open(&source).map_err(|error| {
                        UnivariateMixerError::Ffi {
                            step: "open extract".into(),
                            detail: format!("{source}: {error}"),
                        }
                    })?,
                    &mut output,
                )
                .map_err(|error| UnivariateMixerError::Ffi {
                    step: "copy extract".into(),
                    detail: format!("{source}: {error}"),
                })?;
                output
                    .write_all(b"\n")
                    .map_err(|error| UnivariateMixerError::Ffi {
                        step: "write extract separator".into(),
                        detail: error.to_string(),
                    })?;
            }
            output.flush().map_err(|error| UnivariateMixerError::Ffi {
                step: "flush combined extract".into(),
                detail: error.to_string(),
            })?;
        }
        let extract =
            CString::new(combined_extract_path.to_string_lossy().as_bytes()).map_err(|error| {
                UnivariateMixerError::Ffi {
                    step: "create extract path".into(),
                    detail: error.to_string(),
                }
            })?;

        check(
            &library,
            init(
                context_id,
                bim.as_ptr(),
                empty.as_ptr(),
                chr_labels.as_ptr(),
                trait1.as_ptr(),
                empty.as_ptr(),
                empty.as_ptr(),
                extract.as_ptr(),
                empty.as_ptr(),
            ),
            "bgmg_init",
        )?;
        let initial_num_tag = get_num_tag(context_id);
        if initial_num_tag <= 0 {
            return Err(UnivariateMixerError::Ffi {
                step: "bgmg_init".into(),
                detail: format!(
                    "no tag SNPs remained after reference alignment (num_tag={initial_num_tag})"
                ),
            });
        }

        let seed_option = CString::new("seed").map_err(|error| UnivariateMixerError::Ffi {
            step: "create seed option".into(),
            detail: error.to_string(),
        })?;
        check(
            &library,
            set_option(
                context_id,
                seed_option.as_ptr().cast_mut(),
                seed as c_double,
            ),
            "bgmg_set_option(seed)",
        )?;

        for chromosome in chromosomes {
            let ld_path = ld_template.replace('@', &chromosome.to_string());
            let ld_path = CString::new(ld_path).map_err(|error| UnivariateMixerError::Ffi {
                step: "create LD path".into(),
                detail: error.to_string(),
            })?;
            check(
                &library,
                set_ld_file(context_id, *chromosome, ld_path.as_ptr()),
                "bgmg_set_ld_r2_coo_from_file",
            )?;
            check(
                &library,
                set_ld_csr(context_id, *chromosome),
                "bgmg_set_ld_r2_csr",
            )?;
        }

        check(
            &library,
            set_randprune(context_id, 64, 0.1, 0.0, 0, empty.as_ptr(), empty.as_ptr()),
            "bgmg_set_weights_randprune",
        )?;

        let num_snp = get_num_snp(context_id) as usize;
        let num_tag = get_num_tag(context_id) as usize;
        if num_snp == 0 || num_tag == 0 {
            return Err(UnivariateMixerError::Ffi {
                step: "validate libbgmg context".into(),
                detail: format!("empty context (num_snp={num_snp}, num_tag={num_tag})"),
            });
        }

        let mut tag_indices = vec![0i32; num_tag];
        check(
            &library,
            retrieve_tag_indices(
                context_id,
                num_tag.try_into().map_err(|_| UnivariateMixerError::Ffi {
                    step: "retrieve tag indices".into(),
                    detail: "too many tags".into(),
                })?,
                tag_indices.as_mut_ptr(),
            ),
            "bgmg_retrieve_tag_indices",
        )?;

        let mut maf = vec![0f32; num_snp];
        check(
            &library,
            retrieve_mafvec(
                context_id,
                num_snp.try_into().map_err(|_| UnivariateMixerError::Ffi {
                    step: "retrieve mafvec".into(),
                    detail: "too many SNPs".into(),
                })?,
                maf.as_mut_ptr(),
            ),
            "bgmg_retrieve_mafvec",
        )?;
        if maf.iter().any(|value| !value.is_finite()) {
            return Err(UnivariateMixerError::Ffi {
                step: "validate mafvec".into(),
                detail: "LD file did not provide finite frequencies for every SNP".into(),
            });
        }

        let mut z_tag = vec![0f32; num_tag];
        let mut n_tag = vec![0f32; num_tag];
        let mut weight_tag = vec![0f32; num_tag];
        let trait_length: c_int = num_tag.try_into().map_err(|_| UnivariateMixerError::Ffi {
            step: "retrieve trait vectors".into(),
            detail: "too many tags".into(),
        })?;
        check(
            &library,
            retrieve_zvec(context_id, 1, trait_length, z_tag.as_mut_ptr()),
            "bgmg_retrieve_zvec",
        )?;
        check(
            &library,
            retrieve_nvec(context_id, 1, trait_length, n_tag.as_mut_ptr()),
            "bgmg_retrieve_nvec",
        )?;
        check(
            &library,
            retrieve_weights(
                context_id,
                num_tag.try_into().map_err(|_| UnivariateMixerError::Ffi {
                    step: "retrieve weights".into(),
                    detail: "too many tags".into(),
                })?,
                weight_tag.as_mut_ptr(),
            ),
            "bgmg_retrieve_weights",
        )?;

        let mut z = vec![f64::NAN; num_snp];
        let mut n = vec![0f64; num_snp];
        let mut weights = vec![0f64; num_snp];
        for (&snp, (&z_value, (&n_value, &weight_value))) in tag_indices
            .iter()
            .zip(z_tag.iter().zip(n_tag.iter().zip(weight_tag.iter())))
        {
            let snp = usize::try_from(snp).map_err(|_| UnivariateMixerError::Ffi {
                step: "validate tag index".into(),
                detail: format!("negative SNP index {snp}"),
            })?;
            if snp >= num_snp {
                return Err(UnivariateMixerError::Ffi {
                    step: "validate tag index".into(),
                    detail: format!("tag SNP index {snp} is outside reference size {num_snp}"),
                });
            }
            if !z_value.is_finite() || !n_value.is_finite() {
                return Err(UnivariateMixerError::Ffi {
                    step: "validate trait vector".into(),
                    detail: format!("undefined z/N for tag SNP index {snp}"),
                });
            }
            if !weight_value.is_finite() || weight_value < 0.0 {
                return Err(UnivariateMixerError::Ffi {
                    step: "validate weights".into(),
                    detail: format!("invalid randprune weight for tag SNP index {snp}"),
                });
            }
            z[snp] = z_value as f64;
            n[snp] = n_value as f64;
            weights[snp] = weight_value as f64;
        }

        let mut row_sizes = Vec::with_capacity(num_tag);
        let mut max_row = 0usize;
        for tag_index in 0..num_tag {
            let length = num_ld_tag(
                context_id,
                tag_index
                    .try_into()
                    .map_err(|_| UnivariateMixerError::Ffi {
                        step: "query tag LD size".into(),
                        detail: "too many tags".into(),
                    })?,
            );
            if length < 0 {
                return Err(UnivariateMixerError::Ffi {
                    step: "bgmg_num_ld_r_tag".into(),
                    detail: format!("libbgmg returned {length}"),
                });
            }
            let length = length as usize;
            row_sizes.push(length);
            max_row = max_row.max(length);
        }

        let h: Vec<f64> = maf
            .iter()
            .map(|frequency| 2.0 * (*frequency as f64) * (1.0 - *frequency as f64))
            .collect();
        let mut m1 = vec![0f64; num_snp];
        let mut m2 = vec![0f64; num_snp];
        let mut row_snp_indices = vec![0i32; max_row];
        let mut row_r = vec![0f32; max_row];
        for (tag_index, &length) in row_sizes.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let tag_c: c_int = tag_index
                .try_into()
                .map_err(|_| UnivariateMixerError::Ffi {
                    step: "retrieve tag LD row".into(),
                    detail: "too many tags".into(),
                })?;
            let length_c: c_int = length.try_into().map_err(|_| UnivariateMixerError::Ffi {
                step: "retrieve tag LD row".into(),
                detail: "row is too large".into(),
            })?;
            check(
                &library,
                retrieve_ld_tag(
                    context_id,
                    tag_c,
                    length_c,
                    row_snp_indices.as_mut_ptr(),
                    row_r.as_mut_ptr(),
                ),
                "bgmg_retrieve_ld_r_tag",
            )?;

            let tag_snp = tag_indices[tag_index] as usize;
            let n_tag_value = n[tag_snp];
            for source_index in 0..length {
                let source = usize::try_from(row_snp_indices[source_index]).map_err(|_| {
                    UnivariateMixerError::Ffi {
                        step: "validate LD row".into(),
                        detail: "negative SNP index".into(),
                    }
                })?;
                if source >= num_snp {
                    return Err(UnivariateMixerError::Ffi {
                        step: "validate LD row".into(),
                        detail: format!(
                            "LD SNP index {source} is outside reference size {num_snp}"
                        ),
                    });
                }
                let r2 = row_r[source_index] * row_r[source_index];
                let moment = n_tag_value * h[source] * r2 as f64;
                m1[tag_snp] += moment;
                m2[tag_snp] += moment * moment;
            }
        }

        let totalhet = h.iter().sum();
        Ok(UnivariateSufficient {
            z,
            weights,
            m1,
            m2,
            tags: tag_indices.iter().map(|&snp| snp as u32).collect(),
            totalhet,
            n_snp: num_snp,
        })
    })();

    type DisposeFn = unsafe extern "C" fn(c_int) -> i64;
    let dispose: Symbol<DisposeFn> =
        load_symbol!(&library, b"bgmg_dispose\0", DisposeFn, "load bgmg_dispose")?;
    let dispose_code = dispose(context_id);
    let sufficient = result?;
    check(&library, dispose_code, "bgmg_dispose")?;
    Ok(sufficient)
}

// =====================================================================
// Output builder
// =====================================================================

fn build_result_batch(r: &mixer::result::FitResult) -> Result<RecordBatch, UnivariateMixerError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.params.pi])),
            Arc::new(Float64Array::from(vec![r.params.sig2_beta])),
            Arc::new(Float64Array::from(vec![r.params.sig2_zero])),
            Arc::new(Float64Array::from(vec![r.h2])),
            Arc::new(Float64Array::from(vec![r.nc])),
            Arc::new(Float64Array::from(vec![r.nc_p9])),
            Arc::new(Float64Array::from(vec![r.aic])),
            Arc::new(Float64Array::from(vec![r.bic])),
            Arc::new(Float64Array::from(vec![r.loglike])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Sumstats file writer
// =====================================================================

/// Write selected columns from a DataFrame to a plain TSV file,
/// renaming columns as specified by `col_map`.
/// bgmg auto-detects gz vs plain, so we write an uncompressed TSV.
async fn write_sumstats(
    df: &datafusion::dataframe::DataFrame,
    col_map: &[(&str, &str)], // (source_col, dest_col)
    path: &std::path::Path,
) -> Result<(), UnivariateMixerError> {
    use std::io::Write;

    // Collect the data from the DataFrame
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
        .map_err(|e| UnivariateMixerError::Step {
            context: "select columns".into(),
            detail: e.to_string(),
        })?
        .collect()
        .await
        .map_err(|e| UnivariateMixerError::Step {
            context: "collect batches".into(),
            detail: e.to_string(),
        })?;

    let mut f = std::fs::File::create(path)?;
    use std::io::BufWriter;
    let mut w = BufWriter::new(&mut f);

    // Header
    let header = col_map
        .iter()
        .map(|(_, dst)| *dst)
        .collect::<Vec<_>>()
        .join("\t");
    writeln!(w, "{header}")?;

    // Data rows
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
                    return Err(UnivariateMixerError::Step {
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
    tracing::info!("wrote {} sumstats rows to {}", n_rows, path.display());
    Ok(())
}

/// Count lines in a plain file (for reporting).
fn count_lines(path: &std::path::Path) -> usize {
    use std::io::{BufRead, BufReader};

    std::fs::File::open(path)
        .map(|file| BufReader::new(file).lines().count())
        .unwrap_or(0)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    fn reference_bundle() -> DataBundle {
        DataBundle::new(
            "mixer.g1000_eur",
            "MiXeR reference",
            "/bundles/mixer/g1000_eur",
        )
    }

    use super::*;
    use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
    use datafusion::prelude::CsvReadOptions;

    #[test]
    fn node_type_is_stable() {
        assert_eq!(UNIVARIATE_MIXER_NODE_KIND, "univariate_mixer");
    }

    #[test]
    fn spec_defaults() {
        let spec: UnivariateMixerNodeSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.reference, "g1000_eur");
        assert_eq!(spec.chr2use, "1-22");
        let error =
            serde_json::from_str::<UnivariateMixerNodeSpec>(r#"{"mixer_home":"x"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown field `mixer_home`"));

        let node = UnivariateMixerNode::new(spec, reference_bundle());
        assert_eq!(node.kind(), "univariate_mixer");
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    #[tokio::test]
    async fn write_sumstats_renames_and_stringifies_columns() {
        let ctx = datafusion::prelude::SessionContext::new();
        let batch = RecordBatch::try_new(
            std::sync::Arc::new(Schema::new(vec![
                Field::new(INPUT_RSID_COL, DataType::Utf8, false),
                Field::new(INPUT_A1_COL, DataType::Utf8, true),
                Field::new(INPUT_A2_COL, DataType::Utf8, true),
                Field::new(INPUT_N_COL, DataType::Float64, true),
                Field::new(INPUT_Z_COL, DataType::Float64, true),
            ])),
            vec![
                std::sync::Arc::new(arrow_array::StringArray::from(vec!["rs1"])),
                std::sync::Arc::new(arrow_array::StringArray::from(vec!["A"])),
                std::sync::Arc::new(arrow_array::StringArray::from(vec!["G"])),
                std::sync::Arc::new(Float64Array::from(vec![1000.0])),
                std::sync::Arc::new(Float64Array::from(vec![1.5])),
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
    fn parses_comma_separated_chr2use() {
        assert_eq!(parse_chr2use("21,23-24").unwrap(), vec![21, 23, 24]);
        assert!(parse_chr2use("24-21").is_err());
    }

    #[tokio::test]
    async fn fit1_runs_against_deployed_reference_bundle() {
        if std::env::var("MIXER_NATIVE_IT").ok().as_deref() != Some("1") {
            return;
        }
        let fixture = std::path::Path::new(
            "/mnt/disk3/gsa-mixer/precimed/mixer-test/data/trait1.sumstats.gz",
        );
        let engine_root = std::path::PathBuf::from(
            std::env::var("MIXER_NATIVE_TEST_ROOT")
                .expect("MIXER_NATIVE_TEST_ROOT must point to a staged bundle"),
        );
        if !fixture.is_file() || !engine_root.join("bundle.json").is_file() {
            return;
        }
        let (node_ctx, _vfs_scratch) = crate::mixer_common::tests::vfs_ctx(&engine_root);

        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx
            .read_csv(
                fixture.to_str().unwrap(),
                CsvReadOptions::new()
                    .has_header(true)
                    .delimiter(b'\t')
                    .file_extension("sumstats.gz")
                    .file_compression_type(FileCompressionType::GZIP),
            )
            .await
            .unwrap();
        ctx.register_table("mixer_input", df.into_view()).unwrap();
        let df = ctx
            .sql(r#"SELECT "SNP" AS "rsid", "A1", "A2", "N", "Z" FROM mixer_input"#)
            .await
            .unwrap();
        let mut node = UnivariateMixerNode::new(
            UnivariateMixerNodeSpec {
                reference: "g1000_eur".into(),
                chr2use: "21-22".into(),
                seed: 123,
                diffevo_fast_repeats: 2,
                fast_run: true,
                kmax_pdf: 10,
                downsample_factor: 100,
            },
            reference_bundle(),
        );
        let outputs = node
            .execute(
                &node_ctx,
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
        let value = |name: &str| {
            batches[0]
                .column_by_name(name)
                .unwrap()
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(0)
        };
        assert!((value("pi") - 0.001307).abs() / 0.001307 < 0.02);
        assert!((value("loglike") - 4107.1992).abs() / 4107.1992 < 0.02);
    }
}
