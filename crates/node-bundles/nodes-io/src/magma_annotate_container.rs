//! Containerized MAGMA annotation node using the official static binary.

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
use crate::image_registry::registry_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const MAGMA_ANNOTATE_CONTAINER_KIND: &str = "magma_annotate_container";
pub const MAGMA_ORIGINAL_IMAGE_REPOSITORY: &str = "magma";
pub const MAGMA_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:2ca8540251ee9201f3b7b6ac2596daa2c95bd77eb314305dac61beb9b4d85342";
pub const MAGMA_GENE_LOC_PANEL: &str = "magma.gene_loc.ncbi37_3";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/magma_annotate_container";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MagmaAnnotateContainerSpec {
    /// VFS prefix for immutable annotation output artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct MagmaAnnotateContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl MagmaAnnotateContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MagmaAnnotateContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for MagmaAnnotateContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MagmaAnnotateContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MAGMA_ANNOTATE_CONTAINER_KIND
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

pub fn validate(spec: &MagmaAnnotateContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

pub fn container_spec(spec: &MagmaAnnotateContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: registry_image(
            MAGMA_ORIGINAL_IMAGE_REPOSITORY,
            MAGMA_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["sh".into()],
        script: Some(
            "set -eu\nmagma \\\n  --annotate \\\n  --snp-loc \"$AUTONOMICS_INPUT0\" \\\n  --gene-loc /panels/gene_loc/NCBI37.3.gene.loc \\\n  --out \"$AUTONOMICS_WORKDIR/magma_annotate\" \\\n  > \"$AUTONOMICS_OUTPUT0\" 2>&1"
                .into(),
        ),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "magma_annotate.log".into(),
                format: Some("magma_log".into()),
            },
            ContainerCommandOutputSpec {
                path: "magma_annotate.genes.annot".into(),
                format: Some("magma_genes_annot".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: MAGMA_GENE_LOC_PANEL.into(),
            mount_path: "/panels/gene_loc".into(),
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
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("gene_loc", MAGMA_GENE_LOC_PANEL)]
}

impl NodeFactory for MagmaAnnotateContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MAGMA_ANNOTATE_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official MAGMA v1.10 SNP-to-gene annotation in Podman."
    }

    fn doc(&self) -> &'static str {
        "Runs the official MAGMA v1.10 static executable as an ephemeral Podman \
        container. Input is one whitespace/tab-delimited SNP location File with \
        SNP, chromosome, and base-pair columns. The node binds the official \
        NCBI37.3 gene-location catalog package, invokes `magma --annotate`, and \
        emits the raw MAGMA log plus `.genes.annot` as immutable VFS File \
        artifacts. Window extension is intentionally not exposed because this \
        upstream command annotates against the supplied gene-location table."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaAnnotateContainerSpec)
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
        let spec: MagmaAnnotateContainerSpec = serde_json::from_value(spec)?;
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
        Ok(Box::new(MagmaAnnotateContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MagmaAnnotateContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_official_image_and_panel_contract() {
        let spec = MagmaAnnotateContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        };
        let container = container_spec(&spec).unwrap();
        assert_eq!(
            container.image,
            registry_image(MAGMA_ORIGINAL_IMAGE_REPOSITORY, MAGMA_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, MAGMA_GENE_LOC_PANEL);
        assert_eq!(container.outputs.len(), 2);
        assert!(
            container
                .script
                .as_deref()
                .unwrap()
                .contains("--gene-loc /panels/gene_loc/NCBI37.3.gene.loc")
        );
    }

    #[test]
    fn rejects_relative_artifact_prefix_and_zero_timeout() {
        let mut spec = MagmaAnnotateContainerSpec {
            artifact_prefix: "artifacts/magma".into(),
            timeout_secs: default_timeout_secs(),
        };
        assert!(validate(&spec).is_err());
        spec.artifact_prefix = default_artifact_prefix();
        spec.timeout_secs = 0;
        assert!(validate(&spec).is_err());
    }
}
