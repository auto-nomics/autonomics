//! Containerized MTAG node backed by the original Python implementation.

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

pub const MTAG_CONTAINER_KIND: &str = "mtag_container";
pub const MTAG_ORIGINAL_IMAGE_REPOSITORY: &str = "mtag";
pub const MTAG_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c";
pub const MTAG_LD_REF_PANEL: &str = "mtag.ld_ref.1000g_eur_w_ld";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/mtag_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_TIME_LIMIT_HOURS: f64 = 1.0;
const DEFAULT_TOL: f64 = 1e-6;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MtagContainerSpec {
    /// VFS prefix for immutable MTAG trait results and the raw log.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime, including panel materialization and Job wait.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// MTAG genetic-covariance optimizer time limit in hours.
    #[serde(default = "default_time_limit_hours")]
    pub time_limit_hours: f64,
    /// Relative tolerance passed to the official optimizer.
    #[serde(default = "default_tol")]
    pub tol: f64,
    /// Continue when the official mean-chi-square guard rejects the inputs.
    #[serde(default)]
    pub force: bool,
    /// Assume no sample overlap across traits.
    #[serde(default)]
    pub no_overlap: bool,
    /// Assume perfect genetic covariance.
    #[serde(default)]
    pub perfect_gencov: bool,
    /// Assume equal heritability across traits; requires `perfect_gencov`.
    #[serde(default)]
    pub equal_h2: bool,
    /// Emit standardized official betas instead of unstandardized effects.
    #[serde(default)]
    pub std_betas: bool,
    /// Use the official numerical Omega estimator instead of GMM.
    #[serde(default)]
    pub numerical_omega: bool,
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_time_limit_hours() -> f64 {
    DEFAULT_TIME_LIMIT_HOURS
}

fn default_tol() -> f64 {
    DEFAULT_TOL
}

pub struct MtagContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl MtagContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MtagContainerNode {
    inner: Box<dyn DagNode>,
}

#[async_trait::async_trait]
impl DagNode for MtagContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(MtagContainerNode {
            inner: self.inner.clone_box(),
        })
    }

    fn kind(&self) -> &'static str {
        MTAG_CONTAINER_KIND
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

pub fn validate(spec: &MtagContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.time_limit_hours.is_finite() || spec.time_limit_hours <= 0.0 {
        return Err("time_limit_hours must be finite and greater than zero".into());
    }
    if !spec.tol.is_finite() || spec.tol <= 0.0 {
        return Err("tol must be finite and greater than zero".into());
    }
    if spec.equal_h2 && !spec.perfect_gencov {
        return Err("equal_h2 requires perfect_gencov".into());
    }
    Ok(())
}

pub fn container_spec(spec: &MtagContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let mut flags = format!("--time_limit {} --tol {}", spec.time_limit_hours, spec.tol);
    if spec.force {
        flags.push_str(" --force");
    }
    if spec.no_overlap {
        flags.push_str(" --no_overlap");
    }
    if spec.perfect_gencov {
        flags.push_str(" --perfect_gencov");
    }
    if spec.equal_h2 {
        flags.push_str(" --equal_h2");
    }
    if spec.std_betas {
        flags.push_str(" --std_betas");
    }
    if spec.numerical_omega {
        flags.push_str(" --numerical_omega");
    }

    let script = format!(
        "set -eu\nmtag \\\n  --sumstats \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\" \\\n  --snp_name snpid \\\n  --z_name z \\\n  --n_name n \\\n  --eaf_name freq \\\n  --chr_name chr \\\n  --bpos_name bpos \\\n  --a1_name a1 \\\n  --a2_name a2 \\\n  --ld_ref_panel /panels/ld_ref/ \\\n  --out \"$AUTONOMICS_WORKDIR/mtag\" \\\n  --make_full_path \\\n  {flags}"
    );

    Ok(ContainerCommandSpec {
        image: acr_image(MTAG_ORIGINAL_IMAGE_REPOSITORY, MTAG_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "mtag_trait_1.txt".into(),
                format: Some("mtag_results".into()),
            },
            ContainerCommandOutputSpec {
                path: "mtag_trait_2.txt".into(),
                format: Some("mtag_results".into()),
            },
            ContainerCommandOutputSpec {
                path: "mtag.log".into(),
                format: Some("mtag_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: MTAG_LD_REF_PANEL.into(),
            mount_path: "/panels/ld_ref".into(),
        }],
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
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("ld_ref", MTAG_LD_REF_PANEL)]
}

impl NodeFactory for MtagContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MTAG_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs original MTAG 1.0.8 on two GWAS sumstats in an OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the official Python MTAG 1.0.8 implementation. Both inputs are \
        tab-separated Files with lowercase snpid, a1, a2, freq, chr, bpos, n, \
        z, and p columns. The wrapper fixes the official image digest and the \
        upstream 1000G EUR w-LD catalog panel expected by MTAG's single-prefix \
        LDSC interface. It emits both official trait result tables and the raw \
        MTAG log as immutable VFS File artifacts."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MtagContainerSpec)
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
        let spec: MtagContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(MtagContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MtagContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MtagContainerSpec {
        MtagContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            time_limit_hours: default_time_limit_hours(),
            tol: default_tol(),
            force: false,
            no_overlap: false,
            perfect_gencov: false,
            equal_h2: false,
            std_betas: false,
            numerical_omega: false,
        }
    }

    #[test]
    fn builds_official_two_trait_panel_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(MTAG_ORIGINAL_IMAGE_REPOSITORY, MTAG_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, MTAG_LD_REF_PANEL);
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--sumstats \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\""));
        assert!(script.contains("--ld_ref_panel /panels/ld_ref/"));
        assert_eq!(container.outputs.len(), 3);
    }

    #[test]
    fn rejects_invalid_mtag_parameters() {
        let mut value = spec();
        value.equal_h2 = true;
        value.perfect_gencov = false;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.tol = 0.0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.timeout_secs = 0;
        assert!(validate(&value).is_err());
    }
}
