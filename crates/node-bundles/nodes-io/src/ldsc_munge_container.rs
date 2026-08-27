//! Containerized LDSC summary-statistics munging node.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec, decompress_gzip_inputs,
};
use crate::ldsc_h2_container::LDSC_ORIGINAL_IMAGE;
use container_runtime::{ContainerRuntime, PanelCache, PullPolicy};

pub const LDSC_MUNGE_CONTAINER_KIND: &str = "ldsc_munge_container";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/ldsc_munge_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const DEFAULT_INFO_MIN: f64 = 0.9;
const DEFAULT_MAF_MIN: f64 = 0.01;
const DEFAULT_CHUNKSIZE: usize = 5_000_000;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SignedSumstatsSpec {
    /// Raw column carrying the signed statistic.
    pub column: String,
    /// Value indicating no effect, for example 0 for BETA/Z or 1 for OR.
    pub null_value: f64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LdscMungeContainerSpec {
    /// Optional override for the SNP/rsID column.
    #[serde(default)]
    pub snp: Option<String>,
    /// Optional override for the total sample-size column.
    #[serde(default)]
    pub n_col: Option<String>,
    /// Optional override for the case-count column.
    #[serde(default)]
    pub n_cas_col: Option<String>,
    /// Optional override for the control-count column.
    #[serde(default)]
    pub n_con_col: Option<String>,
    /// Optional override for the effect-allele column.
    #[serde(default)]
    pub a1: Option<String>,
    /// Optional override for the other-allele column.
    #[serde(default)]
    pub a2: Option<String>,
    /// Optional override for the p-value column.
    #[serde(default)]
    pub p: Option<String>,
    /// Optional override for the allele-frequency column.
    #[serde(default)]
    pub frq: Option<String>,
    /// Optional override for a single INFO column.
    #[serde(default)]
    pub info: Option<String>,
    /// Optional overrides for multiple INFO columns. The official CLI filters
    /// on their mean INFO score.
    #[serde(default)]
    pub info_list: Vec<String>,
    /// Optional override for the study-count column.
    #[serde(default)]
    pub nstudy: Option<String>,
    /// Optional explicit signed-statistic interpretation. When omitted,
    /// official munge infers Z, OR, BETA, or LOG_ODDS from the header.
    #[serde(default)]
    pub signed_sumstats: Option<SignedSumstatsSpec>,
    /// Raw columns that official munge should ignore when resolving names.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Fixed total sample size used when the input has no usable N column.
    #[serde(default)]
    pub n: Option<f64>,
    /// Fixed case count used together with `n_con`.
    #[serde(default)]
    pub n_cas: Option<f64>,
    /// Fixed control count used together with `n_cas`.
    #[serde(default)]
    pub n_con: Option<f64>,
    /// Minimum INFO score.
    #[serde(default = "default_info_min")]
    pub info_min: f64,
    /// Minimum MAF.
    #[serde(default = "default_maf_min")]
    pub maf_min: f64,
    /// Explicit minimum sample size. The official default is the input's 90th
    /// percentile N divided by 1.5.
    #[serde(default)]
    pub n_min: Option<f64>,
    /// Explicit minimum study count.
    #[serde(default)]
    pub nstudy_min: Option<f64>,
    /// Parse Stephan Ripke daner format and infer N from FRQ_A_/FRQ_U_ headers.
    #[serde(default)]
    pub daner: bool,
    /// Parse newer daner format with Nca/Nco columns.
    #[serde(default)]
    pub daner_n: bool,
    /// Do not require A1/A2 columns.
    #[serde(default)]
    pub no_alleles: bool,
    /// Interpret A1 as the increasing allele and use the P column for Z.
    #[serde(default)]
    pub a1_inc: bool,
    /// Keep FRQ in the munged output when it is present.
    #[serde(default)]
    pub keep_maf: bool,
    /// Number of rows read per chunk.
    #[serde(default = "default_chunksize")]
    pub chunksize: usize,
    /// VFS prefix for the immutable munged sumstats and log artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_info_min() -> f64 {
    DEFAULT_INFO_MIN
}

fn default_maf_min() -> f64 {
    DEFAULT_MAF_MIN
}

fn default_chunksize() -> usize {
    DEFAULT_CHUNKSIZE
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct LdscMungeContainerNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LdscMungeContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LdscMungeContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for LdscMungeContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for LdscMungeContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LDSC_MUNGE_CONTAINER_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.inner.execute(ctx, inputs, reporter).await
    }
}

fn validate_nonempty_column(name: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} cannot be empty or whitespace"));
    }
    Ok(())
}

fn validate_positive_number(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{name} must be finite and greater than zero"));
    }
    Ok(())
}

pub fn validate(spec: &LdscMungeContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if spec.chunksize == 0 {
        return Err("chunksize must be greater than zero".into());
    }
    if spec.daner && spec.daner_n {
        return Err("daner and daner_n cannot both be true".into());
    }
    if spec.info.is_some() && !spec.info_list.is_empty() {
        return Err("info and info_list are mutually exclusive".into());
    }
    if matches!((spec.n_cas, spec.n_con), (Some(_), None) | (None, Some(_))) {
        return Err("n_cas and n_con must be specified together".into());
    }

    for (name, value) in [
        ("snp", &spec.snp),
        ("n_col", &spec.n_col),
        ("n_cas_col", &spec.n_cas_col),
        ("n_con_col", &spec.n_con_col),
        ("a1", &spec.a1),
        ("a2", &spec.a2),
        ("p", &spec.p),
        ("frq", &spec.frq),
        ("info", &spec.info),
        ("nstudy", &spec.nstudy),
    ] {
        if let Some(value) = value {
            validate_nonempty_column(name, value)?;
        }
    }
    for (index, column) in spec.info_list.iter().enumerate() {
        validate_nonempty_column(&format!("info_list[{index}]"), column)?;
    }
    for (index, column) in spec.ignore.iter().enumerate() {
        validate_nonempty_column(&format!("ignore[{index}]"), column)?;
    }
    if let Some(value) = &spec.signed_sumstats {
        validate_nonempty_column("signed_sumstats.column", &value.column)?;
        if !value.null_value.is_finite() {
            return Err("signed_sumstats.null_value must be finite".into());
        }
    }

    if !(0.0..=2.0).contains(&spec.info_min) || spec.info_min <= 0.0 {
        return Err("info_min must lie in (0, 2]".into());
    }
    if !(0.0..=0.5).contains(&spec.maf_min) || spec.maf_min < 0.0 {
        return Err("maf_min must lie in [0, 0.5]".into());
    }
    for (name, value) in [
        ("n", spec.n),
        ("n_cas", spec.n_cas),
        ("n_con", spec.n_con),
        ("n_min", spec.n_min),
        ("nstudy_min", spec.nstudy_min),
    ] {
        if let Some(value) = value {
            validate_positive_number(name, value)?;
        }
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn push_value_flag(args: &mut Vec<String>, flag: &str, value: &str) {
    args.push(flag.into());
    args.push(shell_quote(value));
}

fn push_number_flag(args: &mut Vec<String>, flag: &str, value: f64) {
    args.push(flag.into());
    args.push(value.to_string());
}

fn command_args(spec: &LdscMungeContainerSpec) -> Vec<String> {
    let mut args = vec![
        "munge_sumstats".to_string(),
        "--sumstats".to_string(),
        "\"$AUTONOMICS_INPUT0\"".to_string(),
    ];

    for (flag, value) in [
        ("--snp", &spec.snp),
        ("--N-col", &spec.n_col),
        ("--N-cas-col", &spec.n_cas_col),
        ("--N-con-col", &spec.n_con_col),
        ("--a1", &spec.a1),
        ("--a2", &spec.a2),
        ("--p", &spec.p),
        ("--frq", &spec.frq),
        ("--info", &spec.info),
        ("--nstudy", &spec.nstudy),
    ] {
        if let Some(value) = value {
            push_value_flag(&mut args, flag, value);
        }
    }
    if !spec.info_list.is_empty() {
        let columns = spec.info_list.join(",");
        push_value_flag(&mut args, "--info-list", &columns);
    }
    if !spec.ignore.is_empty() {
        let columns = spec.ignore.join(",");
        push_value_flag(&mut args, "--ignore", &columns);
    }
    if let Some(value) = &spec.signed_sumstats {
        let signed = format!("{},{}", value.column, value.null_value);
        push_value_flag(&mut args, "--signed-sumstats", &signed);
    }
    for (flag, value) in [
        ("--N", spec.n),
        ("--N-cas", spec.n_cas),
        ("--N-con", spec.n_con),
        ("--info-min", Some(spec.info_min)),
        ("--maf-min", Some(spec.maf_min)),
        ("--n-min", spec.n_min),
        ("--nstudy-min", spec.nstudy_min),
    ] {
        if let Some(value) = value {
            push_number_flag(&mut args, flag, value);
        }
    }
    for (flag, enabled) in [
        ("--daner", spec.daner),
        ("--daner-n", spec.daner_n),
        ("--no-alleles", spec.no_alleles),
        ("--a1-inc", spec.a1_inc),
        ("--keep-maf", spec.keep_maf),
    ] {
        if enabled {
            args.push(flag.into());
        }
    }
    args.push("--chunksize".to_string());
    args.push(spec.chunksize.to_string());
    args
}

pub fn container_spec(spec: &LdscMungeContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let args = command_args(spec).join(" \\\n  ");
    let script = format!(
        "set -eu\n\
         {}\n\
         out_prefix=\"$AUTONOMICS_WORKDIR/munged_sumstats\"\n\
         {args} \\\n\
         \x20 --out \"$out_prefix\" \\\n\
         \x20 > \"$AUTONOMICS_OUTPUT1\" 2>&1\n\
         mv \"$out_prefix.sumstats.gz\" \"$AUTONOMICS_OUTPUT0\"\n\
         cat \"$out_prefix.log\" >> \"$AUTONOMICS_OUTPUT1\"\n",
        decompress_gzip_inputs(1)
    );

    Ok(ContainerCommandSpec {
        image: LDSC_ORIGINAL_IMAGE.into(),
        command: vec!["sh".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "munged.sumstats.gz".into(),
                format: Some("sumstats_gz".into()),
            },
            ContainerCommandOutputSpec {
                path: "munge_sumstats.log".into(),
                format: Some("ldsc_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Never,
        cpus: None,
        memory: None,
        pids_limit: None,
        shm_size: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for LdscMungeContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LDSC_MUNGE_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Munges raw GWAS sumstats with original LDSC in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the original Python LDSC `munge_sumstats.py` CLI from the official \
        LDSC image. Input is one whitespace-delimited raw GWAS summary-statistics \
        File (plain text, gzip, or bzip2 as recognized by official LDSC). The \
        node maps columns, filters INFO/MAF/p-values/alleles, infers or overrides \
        sample size, converts P to signed Z, and emits the official \
        `.sumstats.gz` artifact plus the raw munging log. Port 0 is directly \
        compatible with `ldsc_h2_container` and `ldsc_rg_container`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LdscMungeContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LdscMungeContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::ContainerRuntime> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(LdscMungeContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LdscMungeContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and the local official LDSC image"]
    async fn real_podman_munges_gzip_input_with_official_entrypoint() {
        use container_runtime::{PodmanConfig, PodmanRuntime};
        use flate2::{Compression, write::GzEncoder};
        use std::sync::Arc;

        let objects = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace_root = state.path().join("workspace");
        let workspace = workspace_root.join("run");
        std::fs::create_dir_all(&workspace).unwrap();
        let input_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../containers/ldsc/ldsc-python3/test/munge_test/sumstats");
        let gz_input_path = state.path().join("raw.sumstats.gz");
        {
            let plain = std::fs::read(&input_path).unwrap();
            let mut encoder = GzEncoder::new(
                std::fs::File::create(&gz_input_path).unwrap(),
                Compression::fast(),
            );
            std::io::Write::write_all(&mut encoder, &plain).unwrap();
            encoder.finish().unwrap();
        }
        let storage = Arc::new(vfs::OpendalFileStorage::new(objects.path()));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage),
        );
        let munge_spec = serde_json::from_value::<LdscMungeContainerSpec>(serde_json::json!({
            "daner": true,
            "chunksize": 100,
            "timeout_secs": 120
        }))
        .unwrap();
        let mut container = container_spec(&munge_spec).unwrap();
        container.workdir = Some(workspace.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            container,
            Arc::new(PodmanRuntime::new(PodmanConfig {
                program: "podman".into(),
                workspace_root,
                panel_cache_root: state.path().join("panels"),
            })),
            Arc::new(PanelCache::new(state.path().join("panels"), "")),
        )
        .unwrap();

        let input = dag_core::node::NodeInput::file(
            0,
            dag_core::value::FileRef::local(&gz_input_path, Some("sumstats_gz".into())).unwrap(),
        );
        let outputs = node
            .execute(
                &ctx,
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        assert!(
            outputs
                .get(&0)
                .unwrap()
                .as_file()
                .unwrap()
                .path
                .starts_with("vfs:///artifacts/ldsc_munge_container/")
        );
        let log_file = outputs.get(&1).unwrap().as_file().unwrap();
        let log_path = log_file.path.strip_prefix("vfs://").unwrap();
        let log = ctx
            .opendal
            .as_ref()
            .unwrap()
            .resolve(log_path)
            .read(&ctx.opendal.as_ref().unwrap().resolve_path(log_path))
            .await
            .unwrap();
        let log_bytes = log.to_vec();
        let log = String::from_utf8_lossy(&log_bytes);
        assert!(!log.contains("compression has no effect"), "log:\n{log}");
    }

    fn spec() -> LdscMungeContainerSpec {
        LdscMungeContainerSpec {
            snp: Some("variant".into()),
            n_col: None,
            n_cas_col: None,
            n_con_col: None,
            a1: Some("effect allele".into()),
            a2: Some("other allele".into()),
            p: Some("p.value".into()),
            frq: None,
            info: None,
            info_list: vec![],
            nstudy: None,
            signed_sumstats: Some(SignedSumstatsSpec {
                column: "effect".into(),
                null_value: 0.0,
            }),
            ignore: vec!["odds ratio".into()],
            n: Some(100_000.0),
            n_cas: None,
            n_con: None,
            info_min: default_info_min(),
            maf_min: default_maf_min(),
            n_min: None,
            nstudy_min: None,
            daner: false,
            daner_n: false,
            no_alleles: false,
            a1_inc: false,
            keep_maf: true,
            chunksize: default_chunksize(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_munge_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(container.image, LDSC_ORIGINAL_IMAGE);
        assert!(container.panel_bundles.is_empty());
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.outputs[0].format.as_deref(), Some("sumstats_gz"));
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("prepare_input AUTONOMICS_INPUT0"));
        assert!(script.contains("munge_sumstats \\\n"));
        assert!(
            script.contains("--signed-sumstats \\\n  'effect,0'"),
            "unexpected munge script:\n{script}"
        );
        assert!(script.contains("--a1 \\\n  'effect allele'"));
        assert!(script.contains("--ignore \\\n  'odds ratio'"));
        assert!(script.contains("--keep-maf"));
    }

    #[test]
    fn rejects_conflicting_munge_options() {
        let mut value = spec();
        value.daner = true;
        value.daner_n = true;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.info = Some("info".into());
        value.info_list = vec!["info1".into()];
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.n_cas = Some(1.0);
        assert!(validate(&value).is_err());
    }
}
