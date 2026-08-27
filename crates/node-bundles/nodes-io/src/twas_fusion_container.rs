//! Containerized FUSION TWAS node backed by the official association script.

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

pub const TWAS_FUSION_CONTAINER_KIND: &str = "twas_fusion_container";
pub const FUSION_ORIGINAL_IMAGE_REPOSITORY: &str = "fusion";
pub const FUSION_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:91d11747476967b0131571308f2a64bb12aa459205f282103f6bed4fb69ca0bf";
pub const FUSION_GTEX_V8_PANEL: &str = "fusion.gtex_v8";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/twas_fusion_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_MAX_IMPUTE: f64 = 0.5;
const DEFAULT_MIN_R2PRED: f64 = 0.7;
const DEFAULT_PERM: usize = 0;
const DEFAULT_PERM_MINP: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FusionModel {
    Blup,
    Lasso,
    Top1,
    Enet,
}

impl FusionModel {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Blup => "blup",
            Self::Lasso => "lasso",
            Self::Top1 => "top1",
            Self::Enet => "enet",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TwasFusionContainerSpec {
    /// GTEx v8 tissue name, for example `Whole_Blood`.
    pub tissue: String,
    /// Chromosome to test. FUSION analyzes one chromosome per invocation.
    pub chr: u8,
    /// Force a predictive model instead of selecting the best cross-validation
    /// model. Supported values are `blup`, `lasso`, `top1`, and `enet`.
    #[serde(default)]
    pub force_model: Option<FusionModel>,
    /// Use the `.nofilter.pos` weight manifest instead of the filtered `.pos`.
    #[serde(default)]
    pub use_nofilter_weights: bool,
    /// Maximum fraction of missing LDREF SNPs allowed per feature.
    #[serde(default = "default_max_impute")]
    pub max_impute: f64,
    /// Minimum mean LD imputation accuracy for feature SNP z-scores.
    #[serde(default = "default_min_r2pred")]
    pub min_r2pred: f64,
    /// Maximum permutations per feature; zero disables permutation testing.
    #[serde(default = "default_perm")]
    pub perm: usize,
    /// P-value threshold below which permutation testing is initiated.
    #[serde(default = "default_perm_minp")]
    pub perm_minp: f64,
    /// VFS prefix used to publish immutable FUSION artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_max_impute() -> f64 {
    DEFAULT_MAX_IMPUTE
}

fn default_min_r2pred() -> f64 {
    DEFAULT_MIN_R2PRED
}

fn default_perm() -> usize {
    DEFAULT_PERM
}

fn default_perm_minp() -> f64 {
    DEFAULT_PERM_MINP
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct TwasFusionContainerNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl TwasFusionContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct TwasFusionContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for TwasFusionContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for TwasFusionContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        TWAS_FUSION_CONTAINER_KIND
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

pub fn validate(spec: &TwasFusionContainerSpec) -> Result<(), String> {
    if spec.tissue.trim().is_empty() {
        return Err("tissue cannot be empty".into());
    }
    if spec
        .tissue
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '.'))
    {
        return Err("tissue must be a catalog tissue directory name such as Whole_Blood".into());
    }
    if !(1..=22).contains(&spec.chr) {
        return Err("chr must lie in 1..=22".into());
    }
    if !(0.0..=1.0).contains(&spec.max_impute) || spec.max_impute == 0.0 {
        return Err("max_impute must lie in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.min_r2pred) || spec.min_r2pred == 0.0 {
        return Err("min_r2pred must lie in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.perm_minp) || spec.perm_minp == 0.0 {
        return Err("perm_minp must lie in (0, 1]".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if spec.artifact_prefix.starts_with('/') {
        return Ok(());
    }
    Err("artifact_prefix must be an absolute VFS path".into())
}

fn shell_bool(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

pub fn container_spec(spec: &TwasFusionContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;

    let mut env = std::collections::BTreeMap::from([
        (
            "FUSION_TISSUE_ARCHIVE".to_string(),
            format!("GTExv8.ALL.{}.tar.gz", spec.tissue),
        ),
        ("FUSION_CHR".to_string(), spec.chr.to_string()),
        (
            "FUSION_USE_NOFILTER_WEIGHTS".to_string(),
            shell_bool(spec.use_nofilter_weights).into(),
        ),
        ("FUSION_MAX_IMPUTE".to_string(), spec.max_impute.to_string()),
        ("FUSION_MIN_R2PRED".to_string(), spec.min_r2pred.to_string()),
        ("FUSION_PERM".to_string(), spec.perm.to_string()),
        ("FUSION_PERM_MINP".to_string(), spec.perm_minp.to_string()),
    ]);
    if let Some(model) = spec.force_model {
        env.insert("FUSION_FORCE_MODEL".into(), model.as_arg().into());
    }

    Ok(ContainerCommandSpec {
        image: acr_image(
            FUSION_ORIGINAL_IMAGE_REPOSITORY,
            FUSION_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["/opt/fusion/bin/run_fusion_twas.sh".into()],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "twas_fusion.dat".into(),
                format: Some("fusion_twas".into()),
            },
            ContainerCommandOutputSpec {
                path: "twas_fusion.log".into(),
                format: Some("fusion_twas_log".into()),
            },
            ContainerCommandOutputSpec {
                path: "twas_fusion.mhc.dat".into(),
                format: Some("fusion_twas_mhc".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: FUSION_GTEX_V8_PANEL.into(),
            mount_path: "/panels/fusion_ref".into(),
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
    vec![DataBundleBinding::new("fusion_ref", FUSION_GTEX_V8_PANEL)]
}

impl NodeFactory for TwasFusionContainerNodeFactory {
    fn kind(&self) -> &'static str {
        TWAS_FUSION_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official FUSION TWAS association testing against GTEx v8 weights."
    }

    fn doc(&self) -> &'static str {
        "Runs the official FUSION TWAS FUSION.assoc_test.R script in an \
        ephemeral OCI container. Input is one whitespace-delimited GWAS sumstats \
        File with SNP, A1, A2, and Z; produce it with sink_file using \
        format=\"tsv\". The node mounts the cataloged GTEx v8 weight \
        archives and 1000G EUR LDREF, extracts the selected tissue for one \
        chromosome, and emits the official association table, log, and any \
        separate MHC result. Set tissue (for example Whole_Blood) and chr. \
        By default FUSION selects the best cross-validation model; force_model \
        can pin blup, lasso, top1, or enet."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TwasFusionContainerSpec)
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
        let spec: TwasFusionContainerSpec = serde_json::from_value(spec)?;
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
        Ok(Box::new(TwasFusionContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: TwasFusionContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TwasFusionContainerSpec {
        TwasFusionContainerSpec {
            tissue: "Whole_Blood".into(),
            chr: 21,
            force_model: None,
            use_nofilter_weights: false,
            max_impute: DEFAULT_MAX_IMPUTE,
            min_r2pred: DEFAULT_MIN_R2PRED,
            perm: DEFAULT_PERM,
            perm_minp: DEFAULT_PERM_MINP,
            artifact_prefix: DEFAULT_ARTIFACT_PREFIX.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    #[test]
    fn builds_official_fusion_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                FUSION_ORIGINAL_IMAGE_REPOSITORY,
                FUSION_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(
            container.command,
            vec!["/opt/fusion/bin/run_fusion_twas.sh"]
        );
        assert!(container.script.is_none());
        assert_eq!(
            container.env.get("FUSION_TISSUE_ARCHIVE").unwrap(),
            "GTExv8.ALL.Whole_Blood.tar.gz"
        );
        assert_eq!(container.env.get("FUSION_CHR").unwrap(), "21");
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, FUSION_GTEX_V8_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/fusion_ref");
        assert_eq!(container.outputs.len(), 3);
        assert_eq!(container.network, "isolated");
        assert_eq!(container.pull_policy, PullPolicy::Missing);
    }

    #[test]
    fn force_model_is_passed_safely() {
        let mut value = spec();
        value.force_model = Some(FusionModel::Top1);
        let container = container_spec(&value).unwrap();
        assert_eq!(container.env.get("FUSION_FORCE_MODEL").unwrap(), "top1");
    }

    #[test]
    fn rejects_invalid_parameters() {
        let mut value = spec();
        value.tissue = "../escaped".into();
        assert!(validate(&value).is_err());
        value = spec();
        value.chr = 23;
        assert!(validate(&value).is_err());
        value = spec();
        value.max_impute = 0.0;
        assert!(validate(&value).is_err());
        value = spec();
        value.perm_minp = 0.0;
        assert!(validate(&value).is_err());
        value = spec();
        value.timeout_secs = 0;
        assert!(container_spec(&value).is_err());
    }
}
