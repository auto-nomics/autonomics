//! Containerized MiXeR nodes backed by the official gsa-mixer CLI.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{
    DataBundleBinding, NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs,
    value::PortType,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
    ContainerPanelBundleSpec,
};
use crate::image_registry::acr_image;
use container_runtime::{ContainerRuntime, PanelCache, PullPolicy};

pub const MIXER_FIT1_CONTAINER_KIND: &str = "mixer_fit1_container";
pub const MIXER_FIT2_CONTAINER_KIND: &str = "mixer_fit2_container";
pub const MIXER_ORIGINAL_IMAGE_REPOSITORY: &str = "mixer";
pub const MIXER_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7";
pub const MIXER_G1000_EUR_PANEL: &str = "mixer.g1000_eur";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/mixer_container";
const DEFAULT_TIMEOUT_SECS: u64 = 21_600;
const DEFAULT_SEED: i64 = 123;
const DEFAULT_DIFFEVO_FAST_REPEATS: usize = 20;
const DEFAULT_KMAX_PDF: u32 = 10;
const DEFAULT_DOWNSAMPLE_FACTOR: u32 = 1000;
const DEFAULT_THREADS: u16 = 2;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "16Gi";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MixerContainerSpec {
    /// Chromosomes passed to the official `--chr2use` argument.
    #[serde(default = "default_chr2use")]
    pub chr2use: String,
    /// Random seed passed to the official CLI.
    #[serde(default = "default_seed")]
    pub seed: i64,
    /// Differential-evolution repeats for fast fit sequences.
    #[serde(default = "default_diffevo_fast_repeats")]
    pub diffevo_fast_repeats: usize,
    /// Use the short migration/tutorial fit sequence.
    #[serde(default = "default_true")]
    pub fast_run: bool,
    /// Monte Carlo samples used for PDF/QQ output.
    #[serde(default = "default_kmax_pdf")]
    pub kmax_pdf: u32,
    /// Tag-SNP downsampling factor used by official diagnostics.
    #[serde(default = "default_downsample_factor")]
    pub downsample_factor: u32,
    /// OpenMP threads used by libbgmg.
    #[serde(default = "default_threads")]
    pub threads: u16,
    /// VFS prefix for immutable official result artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_chr2use() -> String {
    "1-22".into()
}

fn default_seed() -> i64 {
    DEFAULT_SEED
}

fn default_diffevo_fast_repeats() -> usize {
    DEFAULT_DIFFEVO_FAST_REPEATS
}

fn default_kmax_pdf() -> u32 {
    DEFAULT_KMAX_PDF
}

fn default_downsample_factor() -> u32 {
    DEFAULT_DOWNSAMPLE_FACTOR
}

fn default_threads() -> u16 {
    DEFAULT_THREADS
}

fn default_true() -> bool {
    true
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub fn validate(spec: &MixerContainerSpec) -> Result<(), String> {
    parse_chr2use(&spec.chr2use)?;
    if spec.seed < 0 || spec.seed > i32::MAX as i64 {
        return Err("seed must be between 0 and 2147483647".into());
    }
    if spec.diffevo_fast_repeats == 0 {
        return Err("diffevo_fast_repeats must be greater than zero".into());
    }
    if spec.kmax_pdf == 0 || spec.downsample_factor == 0 || spec.threads == 0 {
        return Err("kmax_pdf, downsample_factor, and threads must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

fn parse_chr2use(value: &str) -> Result<Vec<u16>, String> {
    let mut chromosomes = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let invalid = || format!("invalid chr2use `{value}`");
        let bounds = part.split_once('-');
        let (start, end) = if let Some((start, end)) = bounds {
            let start: u16 = start.trim().parse().map_err(|_| invalid())?;
            let end: u16 = end.trim().parse().map_err(|_| invalid())?;
            (start, end)
        } else {
            let chromosome: u16 = part.trim().parse().map_err(|_| invalid())?;
            (chromosome, chromosome)
        };
        if start == 0 || end < start || end > 22 {
            return Err(format!("invalid chr2use range `{part}`"));
        }
        chromosomes.extend(start..=end);
    }
    if chromosomes.is_empty() {
        return Err("chr2use cannot be empty".into());
    }
    Ok(chromosomes)
}

fn fit_sequence(fit2: bool, fast_run: bool) -> &'static [&'static str] {
    match (fit2, fast_run) {
        (false, true) => &["diffevo-fast", "neldermead-fast"],
        (false, false) => &["diffevo", "neldermead"],
        (true, true) => &["diffevo-fast", "neldermead-fast"],
        (true, false) => &[
            "diffevo-fast",
            "neldermead-fast",
            "brute1-fast",
            "brent1-fast",
        ],
    }
}

fn command_prefix(spec: &MixerContainerSpec, analysis: &str) -> Vec<String> {
    [
        "python".into(),
        "/tools/mixer/precimed/mixer.py".into(),
        analysis.into(),
        "--bim-file".into(),
        "/panels/mixer_g1000_eur/stage_flat/chr@.bim".into(),
        "--ld-file".into(),
        "/panels/mixer_g1000_eur/ld_mixer/1000G.EUR.chr@".into(),
        "--extract".into(),
        "/panels/mixer_g1000_eur/snps/g1000_eur_chr@.snps".into(),
        "--lib".into(),
        "/tools/mixer/lib/libbgmg.so".into(),
        "--chr2use".into(),
        spec.chr2use.clone(),
        "--seed".into(),
        spec.seed.to_string(),
        "--threads".into(),
        spec.threads.to_string(),
        "--kmax-pdf".into(),
        spec.kmax_pdf.to_string(),
        "--downsample-factor".into(),
        spec.downsample_factor.to_string(),
        "--fit-sequence".into(),
    ]
    .into_iter()
    .chain(
        fit_sequence(analysis == "fit2", spec.fast_run)
            .iter()
            .map(|value| (*value).to_string()),
    )
    .chain([
        "--diffevo-fast-repeats".into(),
        spec.diffevo_fast_repeats.to_string(),
    ])
    .collect()
}

fn script(spec: &MixerContainerSpec, analysis: &str) -> String {
    let mut args = command_prefix(spec, analysis);
    if analysis == "fit1" {
        args.push("--trait1-file".into());
        args.push("$AUTONOMICS_INPUT0".into());
    } else {
        args.extend([
            "--trait1-file".into(),
            "$AUTONOMICS_INPUT0".into(),
            "--trait2-file".into(),
            "$AUTONOMICS_INPUT1".into(),
            "--trait1-params-file".into(),
            "$AUTONOMICS_INPUT2".into(),
            "--trait2-params-file".into(),
            "$AUTONOMICS_INPUT3".into(),
        ]);
    }
    let quoted = args
        .into_iter()
        .map(|value| shell_quote(&value))
        .collect::<Vec<_>>()
        .join(" \\\n  ");
    format!(
        "set -eu\nout_prefix=\"${{AUTONOMICS_OUTPUT0%.json}}\"\n{quoted} \\\n  --out \"$out_prefix\" \\\n  --log \"$AUTONOMICS_OUTPUT1\"\n",
        quoted = quoted
    )
}

fn shell_quote(value: &str) -> String {
    if value.starts_with("$AUTONOMICS_")
        || value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        value.into()
    } else {
        format!("'{value}'")
    }
}

fn container_spec(
    spec: &MixerContainerSpec,
    fit2: bool,
    output_name: &str,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: acr_image(MIXER_ORIGINAL_IMAGE_REPOSITORY, MIXER_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into()],
        script: Some(script(spec, if fit2 { "fit2" } else { "fit1" })),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: format!("{output_name}.json"),
                format: Some("mixer_fit_json".into()),
            },
            ContainerCommandOutputSpec {
                path: format!("{output_name}.log"),
                format: Some("mixer_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: MIXER_G1000_EUR_PANEL.into(),
            mount_path: "/panels/mixer_g1000_eur".into(),
        }],
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: Some(DEFAULT_CPUS),
        memory: Some(DEFAULT_MEMORY.into()),
        pids_limit: Some(512),
        shm_size: None,
        user: None,
    })
}

fn fit1_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn fit2_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new(
        "mixer_g1000_eur",
        MIXER_G1000_EUR_PANEL,
    )]
}

struct MixerInfra {
    runtime: Arc<dyn ContainerRuntime>,
    panel_cache: Arc<PanelCache>,
}

pub struct MixerFit1ContainerNodeFactory {
    infra: MixerInfra,
}

pub struct MixerFit2ContainerNodeFactory {
    infra: MixerInfra,
}

pub struct MixerContainerNode {
    inner: Box<dyn DagNode>,
    kind: &'static str,
}

impl MixerFit1ContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            infra: MixerInfra {
                runtime,
                panel_cache,
            },
        }
    }
}

impl MixerFit2ContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            infra: MixerInfra {
                runtime,
                panel_cache,
            },
        }
    }
}

impl Clone for MixerContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
            kind: self.kind,
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MixerContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        self.kind
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

fn build_node(
    infra: &MixerInfra,
    spec_value: serde_json::Value,
    node_ctx: NodeCtx,
    fit2: bool,
) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
    let spec: MixerContainerSpec = serde_json::from_value(spec_value)?;
    let value = container_spec(&spec, fit2, if fit2 { "mixer_fit2" } else { "mixer_fit1" })
        .map_err(dag_core::registry::error::Error::Unknown)?;
    let bundles = panel_bindings()
        .iter()
        .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
        .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
    let runtime: Arc<dyn container_runtime::ContainerRuntime> = infra.runtime.clone();
    let node = ContainerCommandNode::new_with_catalog_panels(
        value,
        runtime,
        Arc::clone(&infra.panel_cache),
        bundles,
    )
    .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
    Ok(Box::new(MixerContainerNode {
        inner: Box::new(node),
        kind: if fit2 {
            MIXER_FIT2_CONTAINER_KIND
        } else {
            MIXER_FIT1_CONTAINER_KIND
        },
    }))
}

impl NodeFactory for MixerFit1ContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MIXER_FIT1_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official gsa-mixer fit1 in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the official precimed/gsa-mixer v2.2.1 `fit1` CLI. Input is one \
        tab-separated GWAS summary-statistics File with marker, effect allele, \
        other allele, sample size, and signed Z columns; gzip is supported. The \
        node fixes the official image and immutable GRCh37 EUR reference package \
        binding, and emits the raw official fit JSON and complete log as File \
        artifacts. No Rust fit logic is used."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MixerContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        Ok(panel_bindings())
    }

    fn ports(&self) -> NodePorts {
        fit1_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        build_node(&self.infra, spec, node_ctx, false)
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MixerContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(fit1_ports())
    }
}

impl NodeFactory for MixerFit2ContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MIXER_FIT2_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official gsa-mixer fit2 in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the official precimed/gsa-mixer v2.2.1 `fit2` CLI. Inputs are \
        trait1 sumstats, trait2 sumstats, and their respective official fit1 JSON \
        Files, in that order. The node fixes the official image and immutable \
        GRCh37 EUR reference package binding, and emits the raw official bivariate \
        JSON and complete log as File artifacts. No Rust fit logic is used."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MixerContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        Ok(panel_bindings())
    }

    fn ports(&self) -> NodePorts {
        fit2_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        build_node(&self.infra, spec, node_ctx, true)
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MixerContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(fit2_ports())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_official_fit1_contract() {
        let spec = MixerContainerSpec {
            chr2use: default_chr2use(),
            seed: default_seed(),
            diffevo_fast_repeats: default_diffevo_fast_repeats(),
            fast_run: true,
            kmax_pdf: default_kmax_pdf(),
            downsample_factor: default_downsample_factor(),
            threads: default_threads(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        };
        let value = container_spec(&spec, false, "mixer_fit1").unwrap();
        assert_eq!(
            value.image,
            acr_image(MIXER_ORIGINAL_IMAGE_REPOSITORY, MIXER_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(value.pull_policy, PullPolicy::Missing);
        assert_eq!(value.panel_bundles.len(), 1);
        assert_eq!(value.panel_bundles[0].panel_id, MIXER_G1000_EUR_PANEL);
        assert_eq!(value.outputs.len(), 2);
        assert!(value.script.as_deref().unwrap().contains("fit1"));
        assert!(
            value
                .script
                .as_deref()
                .unwrap()
                .contains("/panels/mixer_g1000_eur/ld_mixer/1000G.EUR.chr@")
        );
    }

    #[test]
    fn builds_official_fit2_contract() {
        let spec = MixerContainerSpec {
            fast_run: false,
            ..fit1_test_spec()
        };
        let value = container_spec(&spec, true, "mixer_fit2").unwrap();
        let script = value.script.as_deref().unwrap();
        assert!(script.contains("fit2"));
        assert!(script.contains("--trait2-file"));
        assert!(script.contains("--trait2-params-file"));
        assert!(script.contains("brute1"));
    }

    fn fit1_test_spec() -> MixerContainerSpec {
        MixerContainerSpec {
            chr2use: default_chr2use(),
            seed: default_seed(),
            diffevo_fast_repeats: default_diffevo_fast_repeats(),
            fast_run: true,
            kmax_pdf: default_kmax_pdf(),
            downsample_factor: default_downsample_factor(),
            threads: default_threads(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn rejects_invalid_parameters() {
        let mut spec = fit1_test_spec();
        spec.chr2use = "1-X".into();
        assert!(validate(&spec).is_err());
        spec.chr2use = "23".into();
        assert!(validate(&spec).is_err());
        spec.chr2use = "21-22".into();
        spec.seed = -1;
        assert!(validate(&spec).is_err());
        spec.seed = default_seed();
        spec.kmax_pdf = 0;
        assert!(validate(&spec).is_err());
    }
}
