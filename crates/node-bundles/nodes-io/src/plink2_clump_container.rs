//! Containerized PLINK2 `--clump` node backed by the official PLINK2 binary.
//!
//! This factory owns the tool-image / reference-panel binding so callers
//! select an analysis rather than hand-assembling a `container_command` and
//! guessing which reference panels are compatible. The wrapper is the
//! offline-LD-clumping counterpart to OpenGWAS' remote `/ld/clump` endpoint
//! used by [`nodes_mr::two_sample_mr`]; it satisfies the
//! "offline 1000G PLINK clumping reference for reproducibility" requirement
//! flagged in `docs/container-node-migration-backlog.md` (Wave 1, TwoSampleMR
//! blocker).

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

pub const PLINK2_CLUMP_CONTAINER_KIND: &str = "plink2_clump_container";
pub const PLINK2_ORIGINAL_IMAGE_REPOSITORY: &str = "plink2";
pub const PLINK2_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:998315bf1c34c1c7ef276e93ca6c043dd1982c027c7264325505a9a0adc6d1d2";
pub const PLINK2_REF_BINARY_PANEL: &str = "plink.ref.1000g_eur.binary";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/plink2_clump_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_CLUMP_P1: f64 = 5e-8;
const DEFAULT_CLUMP_P2: f64 = 1e-6;
const DEFAULT_CLUMP_R2: f64 = 0.001;
const DEFAULT_CLUMP_KB: u32 = 10_000;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClumpInputFormat {
    /// TwoSampleMR / OpenGWAS convention: tab-delimited file containing at
    /// minimum an `SNP` column and a `P` (p-value) column.
    Twosamplemr,
    /// OpenGWAS `ieu-b` style: a "variant + pval" CSV with `variant,pval`.
    IeuOpenGWAS,
    /// Tab/space-delimited PLINK1-style association report (CHROM/POS/SNP/P).
    PlinkAssoc,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Plink2ClumpContainerSpec {
    /// P-value threshold for index SNPs (PLINK2 `--clump-p1`). Defaults to
    /// 5e-8 (genome-wide significant).
    #[serde(default = "default_clump_p1")]
    pub clump_p1: f64,
    /// Secondary p-value threshold for clumped SNPs (PLINK2 `--clump-p2`).
    /// Defaults to 1e-6.
    #[serde(default = "default_clump_p2")]
    pub clump_p2: f64,
    /// Maximum LD r² between clumped SNPs (PLINK2 `--clump-r2`).
    /// Defaults to 0.001.
    #[serde(default = "default_clump_r2")]
    pub clump_r2: f64,
    /// Maximum distance (kb) for LD clumping (PLINK2 `--clump-kb`).
    /// Defaults to 10,000.
    #[serde(default = "default_clump_kb")]
    pub clump_kb: u32,
    /// Optional 1-based chromosome selector (`--chr`). When omitted, the
    /// wrapper iterates over autosomes 1..=22.
    #[serde(default)]
    pub chr: Option<u8>,
    /// Input format hint. Determines which `--clump-id-field` and
    /// `--clump-p-field` selectors PLINK2 receives. Defaults to
    /// `twosamplemr`.
    #[serde(default = "default_input_format")]
    pub input_format: ClumpInputFormat,
    /// VFS prefix used to publish immutable clumping artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds (clumping across 22 autosomes can
    /// take up to ~30 minutes on commodity hardware).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_clump_p1() -> f64 {
    DEFAULT_CLUMP_P1
}

fn default_clump_p2() -> f64 {
    DEFAULT_CLUMP_P2
}

fn default_clump_r2() -> f64 {
    DEFAULT_CLUMP_R2
}

fn default_clump_kb() -> u32 {
    DEFAULT_CLUMP_KB
}

fn default_input_format() -> ClumpInputFormat {
    ClumpInputFormat::Twosamplemr
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct Plink2ClumpContainerNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl Plink2ClumpContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct Plink2ClumpContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for Plink2ClumpContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for Plink2ClumpContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        PLINK2_CLUMP_CONTAINER_KIND
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

pub fn validate(spec: &Plink2ClumpContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !(0.0..=1.0).contains(&spec.clump_p1) || !(0.0..=1.0).contains(&spec.clump_p2) {
        return Err("clump_p1 and clump_p2 must lie in (0, 1]".into());
    }
    if spec.clump_p2 < spec.clump_p1 {
        return Err("clump_p2 must be >= clump_p1 (secondary threshold; p1 is the index-SNP threshold, p2 is the looser member threshold)".into());
    }
    if !(0.0..=1.0).contains(&spec.clump_r2) {
        return Err("clump_r2 must lie in (0, 1]".into());
    }
    if spec.clump_kb == 0 {
        return Err("clump_kb must be greater than zero".into());
    }
    if let Some(chr) = spec.chr
        && !(1..=22).contains(&chr)
    {
        return Err("chr must lie in 1..=22 when specified".into());
    }
    Ok(())
}

/// PLINK2 column selectors matching [`ClumpInputFormat`]. PLINK2 can infer
/// common `SNP`/`P` headers, but explicit selectors keep the wrapper contract
/// stable across the three supported report layouts.
fn plink2_clump_field_args(format: &ClumpInputFormat) -> &'static str {
    match format {
        ClumpInputFormat::Twosamplemr | ClumpInputFormat::PlinkAssoc => {
            "--clump-id-field SNP --clump-p-field P"
        }
        ClumpInputFormat::IeuOpenGWAS => "--clump-id-field variant --clump-p-field pval",
    }
}

/// Build the POSIX shell script that drives PLINK2 across one or all
/// autosomes. The script exits non-zero on any per-chromosome failure so
/// that [`ContainerCommandNode`] surfaces the error. The PLINK2 binary lives
/// at `/usr/local/bin/plink2` inside the image (linked from `/opt/plink2`).
fn build_script(spec: &Plink2ClumpContainerSpec) -> String {
    let (seq_arg, chr_flag, label) = match spec.chr {
        Some(c) => (
            format!("{c} {c}"),
            format!("  --chr {c} \\\n"),
            format!("chromosome {c}"),
        ),
        None => (
            "1 22".to_string(),
            String::new(),
            "autosomes 1..=22".to_string(),
        ),
    };
    let clump_field_args = plink2_clump_field_args(&spec.input_format);

    format!(
        "set -eu\n\
         mkdir -p /work/per_chr\n\
         : > \"${{AUTONOMICS_OUTPUT1}}\"\n\
         for chr in $(seq {seq_arg}); do\n\
         \x20 out_prefix=/work/per_chr/clump.chr${{chr}}\n\
         \x20 plink2 \\\n\
         \x20\x20\x20 --bfile /panels/plink_ref/1000G.EUR.QC.${{chr}} \\\n\
         \x20\x20\x20 --clump cols=+chrom,+pos \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 {clump_field_args} \\\n\
         {chr_flag}\
         \x20\x20\x20 --clump-p1 {clump_p1:.0e} \\\n\
         \x20\x20\x20 --clump-p2 {clump_p2:.0e} \\\n\
         \x20\x20\x20 --clump-r2 {clump_r2} \\\n\
         \x20\x20\x20 --clump-kb {clump_kb} \\\n\
         \x20\x20\x20 --out \"${{out_prefix}}\" \\\n\
         \x20\x20\x20>> \"${{AUTONOMICS_OUTPUT0}}\" 2>&1 || {{\n\
         \x20\x20\x20\x20\x20 echo \"plink2 clump failed on chromosome ${{chr}}\" >&2\n\
         \x20\x20\x20\x20\x20 exit 1\n\
         \x20\x20\x20 }}\n\
         \x20 if [ -f \"${{out_prefix}}.clumps\" ]; then\n\
         \x20\x20\x20 sed \"s/^/${{chr}}\\t/\" \"${{out_prefix}}.clumps\" \\\n\
         \x20\x20\x20\x20\x20 >> \"${{AUTONOMICS_OUTPUT1}}\" || true\n\
         \x20 fi\n\
         \x20 printf 'chr\\t%s\\n' \"${{chr}}\" >> \"${{AUTONOMICS_OUTPUT2}}\"\n\
         done\n\
         echo \"plink2 clump completed across {label}\" \\\n\
         \x20 >> \"${{AUTONOMICS_OUTPUT0}}\"\n",
        clump_p1 = spec.clump_p1,
        clump_p2 = spec.clump_p2,
        clump_r2 = spec.clump_r2,
        clump_kb = spec.clump_kb,
        clump_field_args = clump_field_args,
    )
}

pub fn container_spec(spec: &Plink2ClumpContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let script = build_script(spec);
    Ok(ContainerCommandSpec {
        image: acr_image(
            PLINK2_ORIGINAL_IMAGE_REPOSITORY,
            PLINK2_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["sh".into(), "-c".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "plink2_clump.log".into(),
                format: Some("plink2_clump_log".into()),
            },
            ContainerCommandOutputSpec {
                path: "plink2_clump.clumps".into(),
                format: Some("plink2_clumps".into()),
            },
            ContainerCommandOutputSpec {
                path: "plink2_clump.chromosomes.tsv".into(),
                format: Some("plink2_chromosomes".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: PLINK2_REF_BINARY_PANEL.into(),
            mount_path: "/panels/plink_ref".into(),
        }],
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
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
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("plink_ref", PLINK2_REF_BINARY_PANEL)]
}

impl NodeFactory for Plink2ClumpContainerNodeFactory {
    fn kind(&self) -> &'static str {
        PLINK2_CLUMP_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official PLINK2 --clump against 1000G EUR PLINK binary reference."
    }

    fn doc(&self) -> &'static str {
        "Runs the official PLINK2 v2.0.0-a.6.26 binary in an ephemeral k3s \
        Job against the offline 1000G EUR Phase3 PLINK reference panel \
        catalog package `plink.ref.1000g_eur.binary`. Input is one \
        tab-separated sumstats File carrying at least an SNP column and a \
        P column; the wrapper iterates over autosomes 1..=22 by default (or \
        the single chromosome set in the spec). The official `--clumps` \
        artifact and the per-chromosome log are emitted as immutable VFS \
        File artifacts. This wrapper is the offline-LD counterpart to the \
        OpenGWAS `/ld/clump` endpoint used by the native `two_sample_mr` \
        fallback node."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(Plink2ClumpContainerSpec)
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
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: Plink2ClumpContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::ContainerRuntime> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(Plink2ClumpContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: Plink2ClumpContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Plink2ClumpContainerSpec {
        Plink2ClumpContainerSpec {
            clump_p1: default_clump_p1(),
            clump_p2: default_clump_p2(),
            clump_r2: default_clump_r2(),
            clump_kb: default_clump_kb(),
            chr: None,
            input_format: default_input_format(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_plink2_image_and_panel_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                PLINK2_ORIGINAL_IMAGE_REPOSITORY,
                PLINK2_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.network, "isolated");
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, PLINK2_REF_BINARY_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/plink_ref");
        assert_eq!(container.outputs.len(), 3);

        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--bfile /panels/plink_ref/1000G.EUR.QC.${chr}"));
        assert!(script.contains("--clump cols=+chrom,+pos \"${AUTONOMICS_INPUT0}\""));
        assert!(script.contains("--clump-p1 5e-8"));
        assert!(script.contains("--clump-p2 1e-6"));
        assert!(script.contains("--clump-r2 0.001"));
        assert!(script.contains("--clump-kb 10000"));
        assert!(script.contains("seq 1 22"));
        assert!(!script.contains("--chr "));
    }

    #[test]
    fn single_chromosome_renders_chr_flag_and_seq() {
        let mut s = spec();
        s.chr = Some(6);
        let container = container_spec(&s).unwrap();
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("seq 6 6"));
        assert!(script.contains("--chr 6"));
        assert!(!script.contains("seq 1 22"));
    }

    #[test]
    fn rejects_relative_artifact_prefix_and_zero_timeout() {
        let mut s = spec();
        s.artifact_prefix = "artifacts/plink2".into();
        assert!(validate(&s).is_err());
        s.artifact_prefix = default_artifact_prefix();
        s.timeout_secs = 0;
        assert!(validate(&s).is_err());
    }

    #[test]
    fn rejects_inverted_clump_thresholds_and_out_of_range_chr() {
        let mut s = spec();
        s.clump_p1 = 1e-6;
        s.clump_p2 = 5e-8;
        assert!(validate(&s).is_err());
        s.clump_p2 = 1e-6;
        s.clump_kb = 0;
        assert!(validate(&s).is_err());
        s.clump_kb = default_clump_kb();
        s.chr = Some(23);
        assert!(validate(&s).is_err());
        s.chr = Some(0);
        assert!(validate(&s).is_err());
    }

    #[test]
    fn column_override_is_emitted() {
        let container = container_spec(&spec()).unwrap();
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("cols=+chrom,+pos"));
    }

    #[test]
    fn renders_twosamplemr_column_selectors() {
        let container = container_spec(&spec()).unwrap();
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--clump-id-field SNP --clump-p-field P"));
    }

    #[test]
    fn renders_ieu_opengwas_column_selectors() {
        let mut s = spec();
        s.input_format = ClumpInputFormat::IeuOpenGWAS;
        let container = container_spec(&s).unwrap();
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--clump-id-field variant --clump-p-field pval"));
        assert!(!script.contains("--clump-id-field SNP"));
    }

    #[test]
    fn ports_match_three_outputs() {
        let ports = port_layout();
        assert_eq!(ports.input_ports().len(), 1);
        assert_eq!(ports.output_ports().len(), 3);
    }
}
