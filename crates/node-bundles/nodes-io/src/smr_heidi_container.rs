//! Containerized SMR/HEIDI node backed by the official SMR executable.
//!
//! The wrapper owns the image and reference-data contracts for official SMR
//! 1.4.2. The default eQTL contract uses the Westra blood cis-eQTL GRCh37 BESD
//! package; callers can instead select the eQTLGen GRCh37 BESD package. In both
//! cases the LD reference is the 1000G EUR PLINK binary panel. Callers provide
//! one GCTA-COJO `.ma` GWAS summary File plus analysis parameters; they do not
//! assemble mounts.

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
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const SMR_HEIDI_CONTAINER_KIND: &str = "smr_heidi_container";
pub const SMR_ORIGINAL_IMAGE_REPOSITORY: &str = "smr";
pub const SMR_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d";
pub const SMR_WESTRA_EQTL_PANEL: &str = "smr.eqtl.westra_hg19";
pub const SMR_EQTLGEN_PANEL: &str = "smr.eqtl.eqtlgen_hg19";
pub const SMR_REF_BINARY_PANEL: &str = "plink.ref.1000g_eur.binary";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/smr_heidi_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_MAF: f64 = 0.01;
const DEFAULT_PEQTL_SMR: f64 = 5e-8;
const DEFAULT_PEQTL_HEIDI: f64 = 1.5654e-3;
const DEFAULT_HEIDI_MTD: u8 = 1;
const DEFAULT_HEIDI_MIN_M: u32 = 3;
const DEFAULT_HEIDI_MAX_M: u32 = 20;
const DEFAULT_LD_LOWER_LIMIT: f64 = 0.05;
const DEFAULT_LD_UPPER_LIMIT: f64 = 0.9;
const DEFAULT_CIS_WIND_KB: u32 = 2000;
const DEFAULT_MAX_NUM_LD: u32 = 500;
const DEFAULT_THREAD_NUM: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SmrEqtlSource {
    Westra,
    EqtlGen,
}

impl SmrEqtlSource {
    fn panel_id(self) -> &'static str {
        match self {
            Self::Westra => SMR_WESTRA_EQTL_PANEL,
            Self::EqtlGen => SMR_EQTLGEN_PANEL,
        }
    }

    fn besd_prefix(self) -> &'static str {
        match self {
            Self::Westra => "westra_eqtl_hg19",
            Self::EqtlGen => "eqtlgen_hg19",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SmrHeidiContainerSpec {
    /// cis-eQTL summary dataset used as the SMR exposure.
    #[serde(default = "default_eqtl_source")]
    pub eqtl_source: SmrEqtlSource,
    /// Chromosome to analyze. The bound 1000G reference is chromosome-split.
    pub chr: u8,
    /// Minor allele frequency threshold in the LD reference sample.
    #[serde(default = "default_maf")]
    pub maf: f64,
    /// eQTL p-value threshold used to select the top instrument for SMR.
    #[serde(default = "default_peqtl_smr")]
    pub peqtl_smr: f64,
    /// eQTL p-value threshold used to select SNPs for HEIDI.
    #[serde(default = "default_peqtl_heidi")]
    pub peqtl_heidi: f64,
    /// HEIDI method: 0 is the original approach and 1 is the current default.
    #[serde(default = "default_heidi_mtd")]
    pub heidi_mtd: u8,
    /// Minimum number of cis-SNPs required to run HEIDI.
    #[serde(default = "default_heidi_min_m")]
    pub heidi_min_m: u32,
    /// Maximum number of eQTLs used in HEIDI after LD pruning.
    #[serde(default = "default_heidi_max_m")]
    pub heidi_max_m: u32,
    /// Lower LD r-squared pruning limit for HEIDI.
    #[serde(default = "default_ld_lower_limit")]
    pub ld_lower_limit: f64,
    /// Upper LD r-squared pruning limit for HEIDI.
    #[serde(default = "default_ld_upper_limit")]
    pub ld_upper_limit: f64,
    /// cis-eQTL window centered on each probe, in kilobases.
    #[serde(default = "default_cis_wind_kb")]
    pub cis_wind_kb: u32,
    /// Number of top SNPs considered during HEIDI LD pruning.
    #[serde(default = "default_max_num_ld")]
    pub max_num_ld: u32,
    /// Maximum allele-frequency difference allowed between datasets.
    #[serde(default = "default_diff_freq")]
    pub diff_freq: f64,
    /// Maximum proportion of SNPs that may be excluded by allele-frequency QC.
    #[serde(default = "default_diff_freq_prop")]
    pub diff_freq_prop: f64,
    /// OpenMP thread count used by SMR.
    #[serde(default = "default_thread_num")]
    pub thread_num: u32,
    /// VFS prefix used to publish immutable SMR artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Wall-clock timeout for the container.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_maf() -> f64 {
    DEFAULT_MAF
}

fn default_eqtl_source() -> SmrEqtlSource {
    SmrEqtlSource::Westra
}

fn default_peqtl_smr() -> f64 {
    DEFAULT_PEQTL_SMR
}

fn default_peqtl_heidi() -> f64 {
    DEFAULT_PEQTL_HEIDI
}

fn default_heidi_mtd() -> u8 {
    DEFAULT_HEIDI_MTD
}

fn default_heidi_min_m() -> u32 {
    DEFAULT_HEIDI_MIN_M
}

fn default_heidi_max_m() -> u32 {
    DEFAULT_HEIDI_MAX_M
}

fn default_ld_lower_limit() -> f64 {
    DEFAULT_LD_LOWER_LIMIT
}

fn default_ld_upper_limit() -> f64 {
    DEFAULT_LD_UPPER_LIMIT
}

fn default_cis_wind_kb() -> u32 {
    DEFAULT_CIS_WIND_KB
}

fn default_max_num_ld() -> u32 {
    DEFAULT_MAX_NUM_LD
}

fn default_diff_freq() -> f64 {
    0.2
}

fn default_diff_freq_prop() -> f64 {
    0.05
}

fn default_thread_num() -> u32 {
    DEFAULT_THREAD_NUM
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct SmrHeidiContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl SmrHeidiContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct SmrHeidiContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for SmrHeidiContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for SmrHeidiContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        SMR_HEIDI_CONTAINER_KIND
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

fn validate_probability(value: f64, field: &str, allow_zero: bool) -> Result<(), String> {
    if !value.is_finite() || value > 1.0 || (value <= 0.0 && !allow_zero) {
        return Err(format!(
            "{field} must be a finite probability greater than zero"
        ));
    }
    Ok(())
}

pub fn validate(spec: &SmrHeidiContainerSpec) -> Result<(), String> {
    if !(1..=22).contains(&spec.chr) {
        return Err("chr must lie in 1..=22".into());
    }
    if !(0.0..=0.5).contains(&spec.maf) || !spec.maf.is_finite() || spec.maf == 0.0 {
        return Err("maf must lie in (0, 0.5]".into());
    }
    validate_probability(spec.peqtl_smr, "peqtl_smr", false)?;
    validate_probability(spec.peqtl_heidi, "peqtl_heidi", false)?;
    validate_probability(spec.diff_freq, "diff_freq", false)?;
    validate_probability(spec.diff_freq_prop, "diff_freq_prop", false)?;
    if !matches!(spec.heidi_mtd, 0 | 1) {
        return Err("heidi_mtd must be 0 or 1".into());
    }
    if spec.heidi_min_m == 0 || spec.heidi_min_m > spec.heidi_max_m {
        return Err("heidi_min_m must be positive and <= heidi_max_m".into());
    }
    if !(0.0..=1.0).contains(&spec.ld_lower_limit)
        || !(0.0..=1.0).contains(&spec.ld_upper_limit)
        || spec.ld_lower_limit == 0.0
        || spec.ld_upper_limit == 1.0
        || spec.ld_lower_limit >= spec.ld_upper_limit
    {
        return Err("LD limits must satisfy 0 < ld_lower_limit < ld_upper_limit < 1".into());
    }
    if spec.cis_wind_kb == 0 {
        return Err("cis_wind_kb must be greater than zero".into());
    }
    if !(500..=10_000).contains(&spec.max_num_ld) {
        return Err("max_num_ld must lie in 500..=10000".into());
    }
    if spec.thread_num == 0 || spec.thread_num > 1024 {
        return Err("thread_num must lie in 1..=1024".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn build_script(spec: &SmrHeidiContainerSpec) -> String {
    let besd_prefix = spec.eqtl_source.besd_prefix();
    format!(
        "set -eu\n\
         smr \\\n\
         \x20\x20\x20 --bfile /panels/smr_ld_ref/1000G.EUR.QC.{chr} \\\n\
         \x20\x20\x20 --gwas-summary \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 --beqtl-summary /panels/smr_eqtl/{besd_prefix} \\\n\
         \x20\x20\x20 --chr {chr} \\\n\
         \x20\x20\x20 --maf {maf} \\\n\
         \x20\x20\x20 --peqtl-smr {peqtl_smr:e} \\\n\
         \x20\x20\x20 --peqtl-heidi {peqtl_heidi:e} \\\n\
         \x20\x20\x20 --heidi-mtd {heidi_mtd} \\\n\
         \x20\x20\x20 --heidi-min-m {heidi_min_m} \\\n\
         \x20\x20\x20 --heidi-max-m {heidi_max_m} \\\n\
         \x20\x20\x20 --ld-lower-limit {ld_lower_limit} \\\n\
         \x20\x20\x20 --ld-upper-limit {ld_upper_limit} \\\n\
         \x20\x20\x20 --cis-wind {cis_wind_kb} \\\n\
         \x20\x20\x20 --max_num_ld {max_num_ld} \\\n\
         \x20\x20\x20 --diff-freq {diff_freq} \\\n\
         \x20\x20\x20 --diff-freq-prop {diff_freq_prop} \\\n\
         \x20\x20\x20 --thread-num {thread_num} \\\n\
         \x20\x20\x20 --out /work/smr \\\n\
         \x20\x20\x20> \"${{AUTONOMICS_OUTPUT1}}\" 2>&1\n\
         test -s \"${{AUTONOMICS_OUTPUT0}}\"\n",
        chr = spec.chr,
        maf = spec.maf,
        peqtl_smr = spec.peqtl_smr,
        peqtl_heidi = spec.peqtl_heidi,
        heidi_mtd = spec.heidi_mtd,
        heidi_min_m = spec.heidi_min_m,
        heidi_max_m = spec.heidi_max_m,
        ld_lower_limit = spec.ld_lower_limit,
        ld_upper_limit = spec.ld_upper_limit,
        cis_wind_kb = spec.cis_wind_kb,
        max_num_ld = spec.max_num_ld,
        diff_freq = spec.diff_freq,
        diff_freq_prop = spec.diff_freq_prop,
        thread_num = spec.thread_num,
    )
}

pub fn container_spec(spec: &SmrHeidiContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let eqtl_panel_id = spec.eqtl_source.panel_id();
    Ok(ContainerCommandSpec {
        image: acr_image(SMR_ORIGINAL_IMAGE_REPOSITORY, SMR_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into(), "-c".into()],
        script: Some(build_script(spec)),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "smr.smr".into(),
                format: Some("smr_heidi".into()),
            },
            ContainerCommandOutputSpec {
                path: "smr.log".into(),
                format: Some("smr_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![
            ContainerPanelBundleSpec {
                panel_id: eqtl_panel_id.into(),
                mount_path: "/panels/smr_eqtl".into(),
            },
            ContainerPanelBundleSpec {
                panel_id: SMR_REF_BINARY_PANEL.into(),
                mount_path: "/panels/smr_ld_ref".into(),
            },
        ],
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
}

fn panel_bindings_for(eqtl_source: SmrEqtlSource) -> Vec<DataBundleBinding> {
    vec![
        DataBundleBinding::new("smr_eqtl", eqtl_source.panel_id()),
        DataBundleBinding::new("smr_ld_ref", SMR_REF_BINARY_PANEL),
    ]
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    panel_bindings_for(default_eqtl_source())
}

const DESC: &str = "Runs official SMR and HEIDI testing against a selected eQTL dataset.";

const DOC: &str = "Runs the official SMR v1.4.2 executable in an ephemeral Podman \
container. Input is one GCTA-COJO `.ma` GWAS summary File with columns SNP, A1, A2, \
freq, b, se, p, and n; A1/freq must refer to the same effect allele. The node \
mounts an immutable GRCh37 cis-eQTL BESD package selected by `eqtl_source` \
(`westra` by default, or `eqtlgen`) plus the 1000G EUR PLINK binary LD \
reference, analyzes one chromosome per invocation, and emits the official \
`.smr` result plus the complete execution log. The compatibility between the \
Westra image, BESD, and LD-reference versions is recorded in \
`docs/container-node-migration-backlog.md`.";

impl NodeFactory for SmrHeidiContainerNodeFactory {
    fn kind(&self) -> &'static str {
        SMR_HEIDI_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        DESC
    }

    fn doc(&self) -> &'static str {
        DOC
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SmrHeidiContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let spec: SmrHeidiContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(panel_bindings_for(spec.eqtl_source))
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SmrHeidiContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings_for(spec.eqtl_source)
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(SmrHeidiContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: SmrHeidiContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SmrHeidiContainerSpec {
        SmrHeidiContainerSpec {
            eqtl_source: default_eqtl_source(),
            chr: 22,
            maf: default_maf(),
            peqtl_smr: default_peqtl_smr(),
            peqtl_heidi: default_peqtl_heidi(),
            heidi_mtd: default_heidi_mtd(),
            heidi_min_m: default_heidi_min_m(),
            heidi_max_m: default_heidi_max_m(),
            ld_lower_limit: default_ld_lower_limit(),
            ld_upper_limit: default_ld_upper_limit(),
            cis_wind_kb: default_cis_wind_kb(),
            max_num_ld: default_max_num_ld(),
            diff_freq: default_diff_freq(),
            diff_freq_prop: default_diff_freq_prop(),
            thread_num: default_thread_num(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_smr_heidi_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(SMR_ORIGINAL_IMAGE_REPOSITORY, SMR_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.network, "isolated");
        assert_eq!(container.panel_bundles.len(), 2);
        assert_eq!(container.panel_bundles[0].panel_id, SMR_WESTRA_EQTL_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/smr_eqtl");
        assert_eq!(container.panel_bundles[1].panel_id, SMR_REF_BINARY_PANEL);
        assert_eq!(container.panel_bundles[1].mount_path, "/panels/smr_ld_ref");
        assert_eq!(container.outputs.len(), 2);

        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--bfile /panels/smr_ld_ref/1000G.EUR.QC.22"));
        assert!(script.contains("--beqtl-summary /panels/smr_eqtl/westra_eqtl_hg19"));
        assert!(script.contains("--gwas-summary \"${AUTONOMICS_INPUT0}\""));
        assert!(script.contains("--peqtl-smr 5e-8"));
        assert!(script.contains("--peqtl-heidi 1.5654e-3"));
        assert!(script.contains("--max_num_ld 500"));
        assert!(script.contains("--thread-num 1"));
        assert!(script.contains("--out /work/smr"));
        assert!(script.contains("> \"${AUTONOMICS_OUTPUT1}\" 2>&1"));
    }

    #[test]
    fn builds_eqtlgen_smr_heidi_contract() {
        let mut value = spec();
        value.eqtl_source = SmrEqtlSource::EqtlGen;
        let container = container_spec(&value).unwrap();

        assert_eq!(container.panel_bundles.len(), 2);
        assert_eq!(container.panel_bundles[0].panel_id, SMR_EQTLGEN_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/smr_eqtl");
        assert_eq!(container.panel_bundles[1].panel_id, SMR_REF_BINARY_PANEL);
        assert_eq!(container.panel_bundles[1].mount_path, "/panels/smr_ld_ref");

        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--beqtl-summary /panels/smr_eqtl/eqtlgen_hg19"));
        assert!(!script.contains("westra_eqtl_hg19"));

        let bindings = panel_bindings_for(SmrEqtlSource::EqtlGen);
        assert_eq!(bindings[0].bundle_id, SMR_EQTLGEN_PANEL);
        assert_eq!(bindings[1].bundle_id, SMR_REF_BINARY_PANEL);
    }

    #[test]
    fn panel_bindings_match_mount_contract() {
        let bindings = panel_bindings();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].bundle_id, SMR_WESTRA_EQTL_PANEL);
        assert_eq!(bindings[1].bundle_id, SMR_REF_BINARY_PANEL);
    }

    #[test]
    fn defaults_to_westra_and_parses_eqtl_source() {
        let default_spec: SmrHeidiContainerSpec = serde_json::from_value(serde_json::json!({
            "chr": 22
        }))
        .unwrap();
        assert_eq!(default_spec.eqtl_source, SmrEqtlSource::Westra);

        let eqtlgen_spec: SmrHeidiContainerSpec = serde_json::from_value(serde_json::json!({
            "eqtl_source": "eqtlgen",
            "chr": 22
        }))
        .unwrap();
        assert_eq!(eqtlgen_spec.eqtl_source, SmrEqtlSource::EqtlGen);

        assert!(
            serde_json::from_value::<SmrHeidiContainerSpec>(serde_json::json!({
                "eqtl_source": "unknown",
                "chr": 22
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_chromosome_and_timeout() {
        let mut value = spec();
        value.chr = 23;
        assert!(validate(&value).is_err());
        value.chr = 22;
        value.timeout_secs = 0;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_invalid_official_parameters() {
        let mut value = spec();
        value.heidi_mtd = 2;
        assert!(validate(&value).is_err());

        value = spec();
        value.heidi_min_m = 21;
        value.heidi_max_m = 20;
        assert!(validate(&value).is_err());

        value = spec();
        value.ld_lower_limit = 0.95;
        value.ld_upper_limit = 0.9;
        assert!(validate(&value).is_err());

        value = spec();
        value.max_num_ld = 499;
        assert!(validate(&value).is_err());

        value = spec();
        value.peqtl_smr = 0.0;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn ports_match_official_artifacts() {
        let ports = port_layout();
        assert_eq!(ports.input_ports().len(), 1);
        assert_eq!(ports.output_ports().len(), 2);
    }
}
