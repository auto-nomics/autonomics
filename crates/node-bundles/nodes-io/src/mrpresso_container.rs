//! Containerized MR-PRESSO node backed by the official R package.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const MRPRESSO_CONTAINER_KIND: &str = "mrpresso_container";
pub const MRPRESSO_ORIGINAL_IMAGE_REPOSITORY: &str = "mrpresso";
pub const MRPRESSO_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:a3c46770506e07141dd89d3b805a3ec3d04b56367738b81917675c860ad7e2a4";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/mrpresso_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MrpressoContainerSpec {
    pub beta_outcome: String,
    pub sd_outcome: String,
    pub beta_exposure: Vec<String>,
    pub sd_exposure: Vec<String>,
    #[serde(default)]
    pub outlier_test: bool,
    #[serde(default)]
    pub distortion_test: bool,
    #[serde(default = "default_signif_threshold")]
    pub signif_threshold: f64,
    #[serde(default = "default_nb_distribution")]
    pub nb_distribution: usize,
    #[serde(default = "default_seed")]
    pub seed: u32,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_signif_threshold() -> f64 {
    0.05
}

fn default_nb_distribution() -> usize {
    1000
}

fn default_seed() -> u32 {
    123
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct MrpressoContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl MrpressoContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MrpressoContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for MrpressoContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MrpressoContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MRPRESSO_CONTAINER_KIND
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

pub fn validate(spec: &MrpressoContainerSpec) -> Result<(), String> {
    if spec.beta_outcome.trim().is_empty() || spec.sd_outcome.trim().is_empty() {
        return Err("beta_outcome and sd_outcome cannot be empty".into());
    }
    if spec.beta_exposure.is_empty() || spec.beta_exposure.len() != spec.sd_exposure.len() {
        return Err(
            "beta_exposure and sd_exposure must be non-empty and have equal lengths".into(),
        );
    }
    if !(0.0..=1.0).contains(&spec.signif_threshold) {
        return Err("signif_threshold must be in (0, 1]".into());
    }
    if spec.nb_distribution == 0 {
        return Err("nb_distribution must be greater than zero".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn r_string(value: &str) -> String {
    format!("{value:?}")
}

fn r_strings(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| r_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("c({values})")
}

fn r_bool(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

pub fn container_spec(spec: &MrpressoContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let r_code = format!(
        "input <- Sys.getenv(\"AUTONOMICS_INPUT0\")\n\
         result_path <- Sys.getenv(\"AUTONOMICS_OUTPUT0\")\n\
         log_path <- Sys.getenv(\"AUTONOMICS_OUTPUT1\")\n\
         data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)\n\
         set.seed({seed})\n\
         result <- MRPRESSO::mr_presso(\n\
         \x20 BetaOutcome = {beta_outcome},\n\
         \x20 BetaExposure = {beta_exposure},\n\
         \x20 SdOutcome = {sd_outcome},\n\
         \x20 SdExposure = {sd_exposure},\n\
         \x20 OUTLIERtest = {outlier_test},\n\
         \x20 DISTORTIONtest = {distortion_test},\n\
         \x20 data = data,\n\
         \x20 NbDistribution = {nb_distribution},\n\
         \x20 SignifThreshold = {signif_threshold}\n\
         )\n\
         sink(log_path, split = TRUE)\n\
         print(result)\n\
         sink()\n\
         saveRDS(result, result_path)\n",
        seed = spec.seed,
        beta_outcome = r_string(&spec.beta_outcome),
        beta_exposure = r_strings(&spec.beta_exposure),
        sd_outcome = r_string(&spec.sd_outcome),
        sd_exposure = r_strings(&spec.sd_exposure),
        outlier_test = r_bool(spec.outlier_test),
        distortion_test = r_bool(spec.distortion_test),
        nb_distribution = spec.nb_distribution,
        signif_threshold = spec.signif_threshold,
    );

    Ok(ContainerCommandSpec {
        image: acr_image(
            MRPRESSO_ORIGINAL_IMAGE_REPOSITORY,
            MRPRESSO_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["Rscript".into()],
        script: Some(r_code),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "mrpresso.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "mrpresso.log".into(),
                format: Some("mrpresso_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: None,
        memory: None,
        pids_limit: None,
        shm_size: None,
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for MrpressoContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MRPRESSO_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official MR-PRESSO in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the official R MRPRESSO package. Input is one tab-separated File \
        containing outcome effect/SE and at least one exposure effect/SE. The \
        wrapper emits the official result object as an RDS artifact and its \
        printed representation as a log artifact. It executes the official \
        package directly and does not reimplement any numerical logic."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MrpressoContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: MrpressoContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(MrpressoContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MrpressoContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MrpressoContainerSpec {
        MrpressoContainerSpec {
            beta_outcome: "Y_effect".into(),
            sd_outcome: "Y_se".into(),
            beta_exposure: vec!["E1_effect".into()],
            sd_exposure: vec!["E1_se".into()],
            outlier_test: true,
            distortion_test: true,
            signif_threshold: default_signif_threshold(),
            nb_distribution: default_nb_distribution(),
            seed: default_seed(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_mrpresso_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                MRPRESSO_ORIGINAL_IMAGE_REPOSITORY,
                MRPRESSO_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert!(container.panel_bundles.is_empty());
        assert_eq!(container.command, vec!["Rscript".to_string()]);
        assert!(
            container
                .script
                .as_deref()
                .unwrap()
                .contains("MRPRESSO::mr_presso")
        );
        assert_eq!(container.outputs.len(), 2);
    }

    #[test]
    fn rejects_mismatched_exposure_columns() {
        let mut value = spec();
        value.sd_exposure.clear();
        assert!(validate(&value).is_err());
    }
}
