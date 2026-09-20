//! Containerized h² LDSC node backed by the original Python implementation.
//!
//! This factory owns the tool-image/panel binding so callers select an
//! analysis rather than hand-assembling a `container_command` and guessing
//! which reference panels are compatible.

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
    ContainerPanelBundleSpec, decompress_gzip_inputs,
};
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const LDSC_H2_CONTAINER_KIND: &str = "ldsc_h2_container";
pub const LDSC_ORIGINAL_IMAGE_REPOSITORY: &str = "ldsc";
pub const LDSC_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:6ba8941a8f628dcdb8a77c1134b36e9c905495017f1f75616903e70cd32673fe";
pub const LDSC_REF_LD_PANEL: &str = "ldsc.ref_ld.1000g_eur.basic";
pub const LDSC_W_LD_PANEL: &str = "ldsc.w_ld.1000g_eur_hm3_no_mhc";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/ldsc_h2_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const DEFAULT_N_BLOCKS: usize = 200;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LdscH2ContainerSpec {
    /// VFS prefix for the immutable LDSC log artifact.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime, including panel materialization and Job wait.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Block-jackknife block count.
    #[serde(default = "default_n_blocks")]
    pub n_blocks: usize,
    /// Optional constrained LD Score regression intercept.
    #[serde(default)]
    pub intercept_h2: Option<f64>,
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

pub struct LdscH2ContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LdscH2ContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LdscH2ContainerNode {
    inner: Box<dyn DagNode>,
    ports: NodePorts,
}

#[async_trait::async_trait]
impl DagNode for LdscH2ContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LDSC_H2_CONTAINER_KIND
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

impl Clone for LdscH2ContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
            ports: self.ports.clone(),
        }
    }
}

pub fn validate(spec: &LdscH2ContainerSpec) -> Result<(), String> {
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if spec.n_blocks <= 1 {
        return Err("n_blocks must be greater than one".into());
    }
    if let Some(value) = spec.intercept_h2
        && !value.is_finite()
    {
        return Err("intercept_h2 must be finite".into());
    }
    if let Some(value) = spec.chisq_max
        && (!value.is_finite() || value <= 0.0)
    {
        return Err("chisq_max must be finite and greater than zero".into());
    }
    Ok(())
}

pub fn container_spec(spec: &LdscH2ContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let mut flags = format!("--n-blocks {}", spec.n_blocks);
    if let Some(value) = spec.intercept_h2 {
        flags.push_str(&format!(" --intercept-h2 {value}"));
    }
    if let Some(value) = spec.chisq_max {
        flags.push_str(&format!(" --chisq-max {value}"));
    }

    let script = format!(
        "set -eu\n{}ldsc \\\n  --h2 \"$AUTONOMICS_INPUT0\" \\\n  --ref-ld-chr /panels/ref_ld/LDscore. \\\n  --w-ld-chr /panels/w_ld/weights.hm3_noMHC. \\\n  {flags} \\\n  --out \"$AUTONOMICS_WORKDIR/ldsc_h2\" \\\n  > \"$AUTONOMICS_OUTPUT0\" 2>&1",
        decompress_gzip_inputs(1)
    );
    Ok(ContainerCommandSpec {
        image: acr_image(LDSC_ORIGINAL_IMAGE_REPOSITORY, LDSC_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![ContainerCommandOutputSpec {
            path: "ldsc_h2.log".into(),
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
            "sumstats",
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

impl NodeFactory for LdscH2ContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LDSC_H2_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs original LDSC h² analysis on tab-separated sumstats in an OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the original Python LDSC h² estimator as an OCI container. \
        Input must be one tab-separated LDSC sumstats File with SNP, A1, A2, N, \
        and Z columns. Plain `.tsv` and gzip-compressed `.sumstats.gz` are both \
        accepted; LDSC ignores extra columns, but CSV is not accepted. The node \
        owns the compatible original-LDSC image and the EUR reference/w_ld panel \
        bindings; callers do not select or mount panels. The raw LDSC log is \
        emitted as an immutable VFS File artifact. Use `munge_sumstats` separately \
        for raw GWAS inputs until a munging wrapper is registered."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LdscH2ContainerSpec)
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
        let spec: LdscH2ContainerSpec = serde_json::from_value(spec)?;
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
        Ok(Box::new(LdscH2ContainerNode {
            inner: Box::new(node),
            ports: port_layout(),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LdscH2ContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldsc_munge_container::LDSC_MUNGE_CONTAINER_KIND;
    use container_runtime::{ContainerRunRequest, ContainerRuntimeError};

    #[derive(Clone)]
    struct NoopConnection {
        workspace_root: std::path::PathBuf,
    }

    #[async_trait::async_trait]
    impl PodmanConnection for NoopConnection {
        async fn run(
            &self,
            _request: ContainerRunRequest,
        ) -> Result<container_runtime::ContainerRunResult, ContainerRuntimeError> {
            Err(ContainerRuntimeError::Invalid(
                "this connection is only used to build nodes in tests".into(),
            ))
        }

        fn name(&self) -> &'static str {
            "noop-ldsc-build"
        }

        fn workspace_root(&self) -> &std::path::Path {
            &self.workspace_root
        }
    }

    #[test]
    fn builds_a_fixed_panel_and_image_contract() {
        let spec = LdscH2ContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            n_blocks: default_n_blocks(),
            intercept_h2: Some(1.0),
            chisq_max: Some(30.0),
        };
        let container = container_spec(&spec).unwrap();
        assert_eq!(
            container.image,
            acr_image(LDSC_ORIGINAL_IMAGE_REPOSITORY, LDSC_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.panel_bundles.len(), 2);
        assert_eq!(container.panel_bundles[0].panel_id, LDSC_REF_LD_PANEL);
        assert_eq!(container.panel_bundles[1].panel_id, LDSC_W_LD_PANEL);
        assert!(container.script.as_deref().unwrap().contains("--h2"));
        assert!(
            container
                .script
                .as_deref()
                .unwrap()
                .contains("prepare_input AUTONOMICS_INPUT0")
        );
        assert!(
            container
                .script
                .as_deref()
                .unwrap()
                .contains("/panels/ref_ld/LDscore.")
        );
    }

    #[test]
    fn declares_sumstats_input_contract() {
        let ports = port_layout();
        let input = ports.input_port(0).unwrap();
        assert_eq!(input.label.as_deref(), Some("sumstats"));
        assert_eq!(input.format.as_deref(), Some("sumstats_gz"));
        assert!(input.accepts_format(Some("sumstats_tsv")));
        assert!(!input.accepts_format(Some("ldsc_log")));
    }

    #[test]
    fn built_wrapper_keeps_consumer_format_contract() {
        let scratch = tempfile::tempdir().unwrap();
        let mut ref_panel = dag_core::DataBundle::new(
            LDSC_REF_LD_PANEL,
            "reference LD scores",
            "/bundles/ldsc/ref",
        );
        ref_panel.source = Some("/bundles/ldsc/ref".into());
        ref_panel.digest = Some("unused".into());
        let mut w_ld_panel =
            dag_core::DataBundle::new(LDSC_W_LD_PANEL, "LD score weights", "/bundles/ldsc/w_ld");
        w_ld_panel.source = Some("/bundles/ldsc/w_ld".into());
        w_ld_panel.digest = Some("unused".into());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
        .with_bundle_registry(std::sync::Arc::new(
            dag_core::BundleRegistry::from_bundles([ref_panel, w_ld_panel]).unwrap(),
        ));
        let mut registry = dag_core::registry::NodeRegistry::new(ctx);
        registry.register(Box::new(LdscH2ContainerNodeFactory::new(
            std::sync::Arc::new(NoopConnection {
                workspace_root: scratch.path().join("workspace"),
            }),
            std::sync::Arc::new(PanelCache::new(scratch.path().join("panels"))),
        )));
        registry.register(Box::new(
            crate::ldsc_munge_container::LdscMungeContainerNodeFactory::new(
                std::sync::Arc::new(NoopConnection {
                    workspace_root: scratch.path().join("workspace"),
                }),
                std::sync::Arc::new(PanelCache::new(scratch.path().join("panels"))),
            ),
        ));
        let wrapper = registry
            .build_node(LDSC_H2_CONTAINER_KIND, serde_json::json!({}))
            .unwrap();
        let input = wrapper.ports().input_port(0).unwrap();

        assert_eq!(input.format.as_deref(), Some("sumstats_gz"));
        assert!(input.accepts_format(Some("sumstats_gz")));
        assert!(input.accepts_format(Some("sumstats_tsv")));
        assert!(!input.accepts_format(Some("ldsc_log")));

        let mut dag = dag_core::dag::DAG::default();
        dag.add_node(
            "pm".into(),
            registry
                .build_node(LDSC_MUNGE_CONTAINER_KIND, serde_json::json!({}))
                .unwrap(),
        )
        .unwrap();
        dag.add_node("ph".into(), wrapper).unwrap();
        dag.add_edge("pm", "ph", 0, 0).unwrap();

        let mut rejecting = dag_core::dag::DAG::default();
        rejecting
            .add_node(
                "pm".into(),
                registry
                    .build_node(LDSC_MUNGE_CONTAINER_KIND, serde_json::json!({}))
                    .unwrap(),
            )
            .unwrap();
        rejecting
            .add_node(
                "ph".into(),
                registry
                    .build_node(LDSC_H2_CONTAINER_KIND, serde_json::json!({}))
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            rejecting.add_edge("pm", "ph", 1, 0),
            Err(DagError::PortFormatMismatch { .. })
        ));
    }

    #[test]
    fn rejects_invalid_analysis_parameters() {
        let mut spec = LdscH2ContainerSpec {
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            n_blocks: 1,
            intercept_h2: None,
            chisq_max: None,
        };
        assert!(validate(&spec).is_err());
        spec.n_blocks = 200;
        spec.chisq_max = Some(0.0);
        assert!(validate(&spec).is_err());
    }
}
