//! Containerized genetic-correlation node backed by original LDSC.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{
    DataBundleBinding, NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs,
    value::PortType,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::decompress_gzip_inputs;
use crate::container_command::{
    ContainerCommandNode,
    ContainerCommandOutputSpec,
    ContainerCommandSpec,
    ContainerPanelBundleSpec
};
use crate::image_registry::registry_image;
use crate::ldsc_h2_container::{
    LDSC_ORIGINAL_IMAGE_DIGEST, LDSC_ORIGINAL_IMAGE_REPOSITORY, LDSC_REF_LD_PANEL, LDSC_W_LD_PANEL,
};
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const LDSC_RG_CONTAINER_KIND: &str = "ldsc_rg_container";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/ldsc_rg_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const DEFAULT_N_BLOCKS: usize = 200;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LdscRgContainerSpec {
    /// VFS prefix for the immutable LDSC rg log artifact.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Block-jackknife block count.
    #[serde(default = "default_n_blocks")]
    pub n_blocks: usize,
    /// Optional chi-square outlier cutoff.
    #[serde(default)]
    pub chisq_max: Option<f64>,
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_n_blocks() -> usize {
    DEFAULT_N_BLOCKS
}

pub struct LdscRgContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LdscRgContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LdscRgContainerNode {
    inner: Box<dyn DagNode>,
    ports: NodePorts,
}

impl Clone for LdscRgContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
            ports: self.ports.clone(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for LdscRgContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LDSC_RG_CONTAINER_KIND
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

pub fn validate(spec: &LdscRgContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if spec.n_blocks <= 1 {
        return Err("n_blocks must be greater than one".into());
    }
    if let Some(value) = spec.chisq_max
        && (!value.is_finite() || value <= 0.0)
    {
        return Err("chisq_max must be finite and greater than zero".into());
    }
    Ok(())
}

pub fn container_spec(spec: &LdscRgContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let mut flags = format!("--n-blocks {}", spec.n_blocks);
    if let Some(value) = spec.chisq_max {
        flags.push_str(&format!(" --chisq-max {value}"));
    }

    Ok(ContainerCommandSpec {
        image: registry_image(LDSC_ORIGINAL_IMAGE_REPOSITORY, LDSC_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into()],
        script: Some(format!(
            "set -eu\n{}ldsc \\\n  --rg \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\" \\\n  --ref-ld-chr /panels/ref_ld/LDscore. \\\n  --w-ld-chr /panels/w_ld/weights.hm3_noMHC. \\\n  {flags} \\\n  --out \"$AUTONOMICS_WORKDIR/ldsc_rg\" \\\n  > \"$AUTONOMICS_OUTPUT0\" 2>&1",
            decompress_gzip_inputs(2)
        )),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![ContainerCommandOutputSpec {
            path: "ldsc_rg.log".into(),
            format: Some("ldsc_log".into()),
        }],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![
            ContainerPanelBundleSpec {
                panel_id: LDSC_REF_LD_PANEL.into(),
                mount_path: "/panels/ref_ld".into(),
            },
            ContainerPanelBundleSpec {
                panel_id: LDSC_W_LD_PANEL.into(),
                mount_path: "/panels/w_ld".into(),
            },
        ],
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
        .add_input_port_of_type_with_accepted_formats(
            None,
            PortType::File,
            "trait_1_sumstats",
            "sumstats_gz",
            ["sumstats_gz", "sumstats_tsv"],
        )
        .add_input_port_of_type_with_accepted_formats(
            None,
            PortType::File,
            "trait_2_sumstats",
            "sumstats_gz",
            ["sumstats_gz", "sumstats_tsv"],
        )
        .add_output_port_of_type_with_label_and_format(None, PortType::File, "log", "ldsc_log")
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![
        DataBundleBinding::new("ref_ld", LDSC_REF_LD_PANEL),
        DataBundleBinding::new("w_ld", LDSC_W_LD_PANEL),
    ]
}

impl NodeFactory for LdscRgContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LDSC_RG_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs original LDSC genetic correlation in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the original Python LDSC `--rg` estimator as an OCI container. Both \
        inputs must be tab-separated LDSC sumstats Files with SNP, A1, A2, N, \
        and Z columns; plain `.tsv` and gzip-compressed `.sumstats.gz` are both \
        accepted. The node owns the compatible image and EUR reference/w_ld panel \
        bindings and emits the raw LDSC log as an immutable VFS File artifact."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LdscRgContainerSpec)
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
        let spec: LdscRgContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            self.kind(),
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(LdscRgContainerNode {
            inner: Box::new(node),
            ports: port_layout(),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LdscRgContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_two_input_official_ldsc_contract() {
        let spec = LdscRgContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            n_blocks: default_n_blocks(),
            chisq_max: Some(30.0),
        };
        let container = container_spec(&spec).unwrap();
        assert_eq!(
            container.image,
            registry_image(LDSC_ORIGINAL_IMAGE_REPOSITORY, LDSC_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.panel_bundles.len(), 2);
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("--rg \"$AUTONOMICS_INPUT0\",\"$AUTONOMICS_INPUT1\""));
        assert!(script.contains("prepare_input AUTONOMICS_INPUT0"));
        assert!(script.contains("prepare_input AUTONOMICS_INPUT1"));
        assert!(script.contains("/panels/ref_ld/LDscore."));
    }

    #[test]
    fn rejects_invalid_rg_parameters() {
        let mut spec = LdscRgContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            n_blocks: 1,
            chisq_max: None,
        };
        assert!(validate(&spec).is_err());
        spec.n_blocks = 200;
        spec.chisq_max = Some(0.0);
        assert!(validate(&spec).is_err());
    }
}
