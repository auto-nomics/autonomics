//! Containerized HDL-L node backed by the official R package.
//!
//! The wrapper invokes `HDL::HDL.L()` for one official `chr`/`piece` block.
//! The UKB LD SVD and BIM payloads are mounted from one immutable catalog
//! package that is separate from the LAVA reference contract.

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
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const HDL_L_CONTAINER_KIND: &str = "hdl_l_container";
pub const HDL_ORIGINAL_IMAGE: &str = "localhost/atc/hdl:1.4.3";
pub const HDL_ORIGINAL_IMAGE_MANIFEST_DIGEST: &str =
    "sha256:9d562d48b805f1b361060a2ca36fe95e7b4227268c7c17ea15006b770d8d30aa";
pub const HDL_UKB_EUR_PANEL: &str = "hdl.ref.ukb_eur";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/hdl_l_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_NREF: f64 = 335_272.0;
const DEFAULT_EIGEN_CUT: f64 = 0.99;
const DEFAULT_ALPHA: f64 = 0.05;
const DEFAULT_LIM: f64 = 1.522997974471263e-8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MissingSampleSizePolicy {
    Median,
    Min,
    Max,
}

impl MissingSampleSizePolicy {
    pub(crate) fn r_argument(&self) -> &'static str {
        match self {
            Self::Median => "median",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HdlLContainerSpec {
    /// Official LD block chromosome, 1 through 22.
    pub chr: u8,
    /// Official one-based UKB LD block number.
    pub piece: u32,
    /// Trait label for summary-stat input port 0.
    pub trait1_name: String,
    /// Trait label for summary-stat input port 1.
    pub trait2_name: String,
    /// Sample overlap between the two cohorts.
    #[serde(default)]
    pub n0: f64,
    /// LD-reference sample size.
    #[serde(default = "default_nref")]
    pub nref: f64,
    /// Cumulative eigenvalue fraction retained by the official method.
    #[serde(default = "default_eigen_cut")]
    pub eigen_cut: f64,
    /// Significance level for likelihood-based confidence intervals.
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    /// Positive likelihood tolerance used by the official optimizer.
    #[serde(default = "default_lim")]
    pub lim: f64,
    /// Include the official intercept rows in the flattened TSV.
    #[serde(default)]
    pub intercept_output: bool,
    /// Fill missing sample sizes instead of dropping those variants.
    #[serde(default)]
    pub fill_missing_n: Option<MissingSampleSizePolicy>,
    /// VFS prefix used to publish immutable HDL-L artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Wall-clock timeout for the container.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_nref() -> f64 {
    DEFAULT_NREF
}

fn default_eigen_cut() -> f64 {
    DEFAULT_EIGEN_CUT
}

fn default_alpha() -> f64 {
    DEFAULT_ALPHA
}

fn default_lim() -> f64 {
    DEFAULT_LIM
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct HdlLContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl HdlLContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct HdlLContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for HdlLContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for HdlLContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        HDL_L_CONTAINER_KIND
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

fn r_string(value: &str) -> String {
    format!("{value:?}")
}

fn r_bool(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

pub fn validate(spec: &HdlLContainerSpec) -> Result<(), String> {
    if !(1..=22).contains(&spec.chr) {
        return Err("chr must be between 1 and 22".into());
    }
    if spec.piece == 0 {
        return Err("piece must be a positive one-based LD block".into());
    }
    if spec.trait1_name.trim().is_empty() || spec.trait2_name.trim().is_empty() {
        return Err("trait1_name and trait2_name cannot be empty".into());
    }
    if !spec.n0.is_finite() || spec.n0 < 0.0 {
        return Err("n0 must be finite and non-negative".into());
    }
    if !spec.nref.is_finite() || spec.nref <= 0.0 {
        return Err("nref must be finite and greater than zero".into());
    }
    if !spec.eigen_cut.is_finite() || !(0.0..=1.0).contains(&spec.eigen_cut) {
        return Err("eigen_cut must be finite and in [0, 1]".into());
    }
    if !spec.alpha.is_finite() || !(0.0..=1.0).contains(&spec.alpha) || spec.alpha == 0.0 {
        return Err("alpha must be finite and in (0, 1]".into());
    }
    if !spec.lim.is_finite() || spec.lim <= 0.0 {
        return Err("lim must be finite and greater than zero".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn build_script(spec: &HdlLContainerSpec) -> String {
    let fill_missing_n = spec
        .fill_missing_n
        .map(|policy| r_string(policy.r_argument()))
        .unwrap_or_else(|| "NULL".into());
    format!(
        r#"
options(stringsAsFactors = FALSE, warn = 1)

read_sumstats <- function(path) {{
  frame <- as.data.frame(data.table::fread(path, sep = "auto", header = TRUE))
  required <- c("SNP", "A1", "A2", "N")
  missing <- setdiff(required, colnames(frame))
  if (length(missing) > 0) {{
    stop("summary statistics are missing required columns: ",
         paste(missing, collapse = ", "))
  }}
  if (!"Z" %in% colnames(frame) &&
      !all(c("b", "se") %in% colnames(frame))) {{
    stop("summary statistics must contain Z or both b and se")
  }}
  frame
}}

ld_path <- "/panels/hdl_ref/LD/"
bim_path <- "/panels/hdl_ref/bim/"
if (!file.exists(file.path(ld_path, "HDLL_LOC_snps.RData"))) {{
  stop("HDL panel is missing LD/HDLL_LOC_snps.RData")
}}
if (length(list.files(bim_path, pattern = "\\.bim$")) == 0) {{
  stop("HDL panel contains no BIM files")
}}

gwas1 <- read_sumstats(Sys.getenv("AUTONOMICS_INPUT0"))
gwas2 <- read_sumstats(Sys.getenv("AUTONOMICS_INPUT1"))
result <- HDL::HDL.L(
  gwas1.df = gwas1,
  gwas2.df = gwas2,
  Trait1name = {},
  Trait2name = {},
  LD.path = ld_path,
  bim.path = bim_path,
  Nref = {},
  N0 = {},
  chr = {},
  piece = {},
  output.file = Sys.getenv("AUTONOMICS_OUTPUT2"),
  eigen.cut = {},
  intercept.output = {},
  fill.missing.N = {},
  lim = {},
  alpha = {}
)
if (is.null(result) || nrow(result) == 0) {{
  stop("official HDL-L returned no result")
}}
print(result)
write.table(
  result,
  Sys.getenv("AUTONOMICS_OUTPUT0"),
  sep = "\t",
  quote = FALSE,
  row.names = FALSE
)
saveRDS(result, Sys.getenv("AUTONOMICS_OUTPUT1"))
"#,
        r_string(&spec.trait1_name),
        r_string(&spec.trait2_name),
        spec.nref,
        spec.n0,
        spec.chr,
        spec.piece,
        spec.eigen_cut,
        r_bool(spec.intercept_output),
        fill_missing_n,
        spec.lim,
        spec.alpha,
    )
}

pub fn container_spec(spec: &HdlLContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: HDL_ORIGINAL_IMAGE.into(),
        command: vec!["Rscript".into()],
        script: Some(build_script(spec)),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "hdl_l.tsv".into(),
                format: Some("hdl_l_result_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "hdl_l.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "hdl_l.log".into(),
                format: Some("hdl_l_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: HDL_UKB_EUR_PANEL.into(),
            mount_path: "/panels/hdl_ref".into(),
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
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("hdl_ref", HDL_UKB_EUR_PANEL)]
}

const DOC: &str = "Runs official HDL-L (R HDL 1.4.3) for one chr/piece block in \
an ephemeral OCI container. Input port 0 and port 1 are the two GWAS summary \
Files in official HDL-L format; each must contain SNP, A1, A2, and N plus \
either Z or b and se. The node mounts the dedicated UKB EUR LD SVD/BIM panel \
from `hdl.ref.ukb_eur` and emits a flattened TSV, the raw official result as \
RDS, and the complete official log. The LAVA and generic 1000G PLINK panels \
are not interchangeable with this contract.";

impl NodeFactory for HdlLContainerNodeFactory {
    fn kind(&self) -> &'static str {
        HDL_L_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs the official HDL R package for one LD block in Podman."
    }

    fn doc(&self) -> &'static str {
        DOC
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HdlLContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let spec: HdlLContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
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
        let spec: HdlLContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
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
        Ok(Box::new(HdlLContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: HdlLContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HdlLContainerSpec {
        HdlLContainerSpec {
            chr: 1,
            piece: 3,
            trait1_name: "Basal metabolic rate".into(),
            trait2_name: "Standing height".into(),
            n0: 0.0,
            nref: default_nref(),
            eigen_cut: default_eigen_cut(),
            alpha: default_alpha(),
            lim: default_lim(),
            intercept_output: false,
            fill_missing_n: None,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_hdl_l_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(container.image, HDL_ORIGINAL_IMAGE);
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, HDL_UKB_EUR_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/hdl_ref");
        assert_eq!(container.outputs.len(), 3);

        let script = container.script.as_deref().unwrap();
        assert!(script.contains("HDL::HDL.L("));
        assert!(script.contains(r#"Trait1name = "Basal metabolic rate""#));
        assert!(script.contains("ld_path <- \"/panels/hdl_ref/LD/\""));
        assert!(script.contains("bim_path <- \"/panels/hdl_ref/bim/\""));
        assert!(script.contains("HDLL_LOC_snps.RData"));
        assert!(script.contains("data.table::fread"));
        assert!(script.contains("write.table("));
        assert!(script.contains("saveRDS(result"));
    }

    #[test]
    fn panel_binding_is_not_shared_with_lava() {
        let bindings = panel_bindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].bundle_id, HDL_UKB_EUR_PANEL);
        assert_ne!(HDL_UKB_EUR_PANEL, crate::lava_container::LAVA_UKB_EUR_PANEL);
    }

    #[test]
    fn validates_official_block_and_likelihood_parameters() {
        assert!(validate(&spec()).is_ok());

        for chr in [0, 23] {
            let mut value = spec();
            value.chr = chr;
            assert!(validate(&value).is_err());
        }

        let mut value = spec();
        value.piece = 0;
        assert!(validate(&value).is_err());

        value = spec();
        value.nref = 0.0;
        assert!(validate(&value).is_err());

        value = spec();
        value.eigen_cut = 1.01;
        assert!(validate(&value).is_err());

        value = spec();
        value.alpha = 0.0;
        assert!(validate(&value).is_err());
    }
}
