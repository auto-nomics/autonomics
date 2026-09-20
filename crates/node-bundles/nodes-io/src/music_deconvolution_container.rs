//! Containerized MuSiC reference-based bulk RNA-seq deconvolution.
//!
//! The wrapper owns the file contract but deliberately owns no reference
//! dataset. Bulk expression, a gene-by-cell single-cell reference, and cell
//! metadata are required inputs; cell sizes and marker genes are optional.

use std::collections::BTreeMap;
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

pub const MUSIC_DECONVOLUTION_CONTAINER_KIND: &str = "music_deconvolution_container";
pub const MUSIC_DECONVOLUTION_IMAGE_REPOSITORY: &str = "music-deconvolution";
pub const MUSIC_DECONVOLUTION_IMAGE_DIGEST: &str =
    "sha256:311e32ba2b0e0ce81715bc72f17342604086faeb7eb4964f706f83dcff53c971";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/music_deconvolution_container";
const DEFAULT_TIMEOUT_SECS: u64 = 7200;
const DEFAULT_CPUS: f64 = 4.0;
const DEFAULT_MEMORY: &str = "12Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;
const DEFAULT_SHM_SIZE: &str = "1Gi";

fn default_cell_type_col() -> String {
    "cell_type".into()
}

fn default_subject_col() -> String {
    "subject_id".into()
}

fn default_iter_max() -> u32 {
    1000
}

fn default_nu() -> f64 {
    0.0001
}

fn default_epsilon() -> f64 {
    0.01
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MusicDeconvolutionContainerSpec {
    #[serde(default = "default_cell_type_col")]
    pub cell_type_col: String,
    #[serde(default = "default_subject_col")]
    pub subject_col: String,
    #[serde(default)]
    pub select_cell_types: Vec<String>,
    #[serde(default = "default_iter_max")]
    pub iter_max: u32,
    #[serde(default = "default_nu")]
    pub nu: f64,
    #[serde(default = "default_epsilon")]
    pub epsilon: f64,
    #[serde(default)]
    pub centered: bool,
    #[serde(default)]
    pub normalize: bool,
    #[serde(default)]
    pub ct_cov: bool,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

pub struct MusicDeconvolutionContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl MusicDeconvolutionContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MusicDeconvolutionContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for MusicDeconvolutionContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MusicDeconvolutionContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MUSIC_DECONVOLUTION_CONTAINER_KIND
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

pub fn validate(spec: &MusicDeconvolutionContainerSpec) -> Result<(), String> {
    for (name, value) in [
        ("cell_type_col", &spec.cell_type_col),
        ("subject_col", &spec.subject_col),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
        if !is_simple_r_identifier(value) {
            return Err(format!("{name} must be a simple R identifier"));
        }
    }
    let mut cell_types = spec.select_cell_types.clone();
    cell_types.sort();
    if cell_types.windows(2).any(|window| window[0] == window[1]) {
        return Err("select_cell_types cannot contain duplicates".into());
    }
    for cell_type in &spec.select_cell_types {
        if cell_type.trim().is_empty() {
            return Err("select_cell_types cannot contain empty values".into());
        }
    }
    if spec.iter_max == 0 {
        return Err("iter_max must be greater than zero".into());
    }
    if !spec.nu.is_finite() || spec.nu <= 0.0 {
        return Err("nu must be finite and greater than zero".into());
    }
    if !spec.epsilon.is_finite() || spec.epsilon <= 0.0 {
        return Err("epsilon must be finite and greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

fn is_simple_r_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '.') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

pub fn container_spec(
    spec: &MusicDeconvolutionContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_MUSIC_CELL_TYPE_COL".to_string(),
            spec.cell_type_col.clone(),
        ),
        (
            "AUTONOMICS_MUSIC_SUBJECT_COL".to_string(),
            spec.subject_col.clone(),
        ),
        (
            "AUTONOMICS_MUSIC_SELECT_CELL_TYPES".to_string(),
            spec.select_cell_types.join(","),
        ),
        (
            "AUTONOMICS_MUSIC_ITER_MAX".to_string(),
            spec.iter_max.to_string(),
        ),
        ("AUTONOMICS_MUSIC_NU".to_string(), spec.nu.to_string()),
        (
            "AUTONOMICS_MUSIC_EPSILON".to_string(),
            spec.epsilon.to_string(),
        ),
        (
            "AUTONOMICS_MUSIC_CENTERED".to_string(),
            spec.centered.to_string(),
        ),
        (
            "AUTONOMICS_MUSIC_NORMALIZE".to_string(),
            spec.normalize.to_string(),
        ),
        (
            "AUTONOMICS_MUSIC_CT_COV".to_string(),
            spec.ct_cov.to_string(),
        ),
    ]);

    Ok(ContainerCommandSpec {
        image: acr_image(
            MUSIC_DECONVOLUTION_IMAGE_REPOSITORY,
            MUSIC_DECONVOLUTION_IMAGE_DIGEST,
        )?,
        command: vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/music_runner.R".to_string(),
        ],
        script: None,
        files: BTreeMap::new(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "cell_type_proportions.tsv".into(),
                format: Some("music_proportions_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "nnls_proportions.tsv".into(),
                format: Some("music_nnls_proportions_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "gene_weights.tsv".into(),
                format: Some("music_gene_weights_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "diagnostics.tsv".into(),
                format: Some("music_diagnostics_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "run_report.json".into(),
                format: Some("music_run_report_json".into()),
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
        cpus: Some(DEFAULT_CPUS),
        memory: Some(DEFAULT_MEMORY.into()),
        pids_limit: Some(DEFAULT_PIDS_LIMIT),
        shm_size: Some(DEFAULT_SHM_SIZE.into()),
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "bulk_expression")
        .add_input_port_of_type_with_label(None, PortType::File, "single_cell_expression")
        .add_input_port_of_type_with_label(None, PortType::File, "cell_metadata")
        .add_optional_input_port_of_type(PortType::File)
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for MusicDeconvolutionContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MUSIC_DECONVOLUTION_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs pinned MuSiC reference-based bulk RNA-seq deconvolution in an isolated container."
    }

    fn doc(&self) -> &'static str {
        "Input ports are a gene-by-bulk-sample expression matrix, a \
        gene-by-cell single-cell count matrix, and cell metadata beginning \
        with `cell_id`. Optional ports supply cell_type/cell_size values and a \
        marker gene list. The node constructs a SingleCellExperiment, runs \
        official MuSiC::music_prop using the configured subject and cell-type \
        columns, and publishes MuSiC proportions, NNLS proportions, gene \
        weights, diagnostics and a provenance report. No reference panel is \
        embedded in the image."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MusicDeconvolutionContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: MusicDeconvolutionContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(MusicDeconvolutionContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MusicDeconvolutionContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MusicDeconvolutionContainerSpec {
        MusicDeconvolutionContainerSpec {
            cell_type_col: default_cell_type_col(),
            subject_col: default_subject_col(),
            select_cell_types: vec!["neuron".into(), "astrocyte".into()],
            iter_max: default_iter_max(),
            nu: default_nu(),
            epsilon: default_epsilon(),
            centered: false,
            normalize: false,
            ct_cov: false,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_music_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                MUSIC_DECONVOLUTION_IMAGE_REPOSITORY,
                MUSIC_DECONVOLUTION_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(
            container.command,
            vec![
                "Rscript".to_string(),
                "--vanilla".to_string(),
                "/opt/autonomics/music_runner.R".to_string()
            ]
        );
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert!(container.panel_bundles.is_empty());
        assert_eq!(container.outputs.len(), 5);
        assert_eq!(container.env["AUTONOMICS_MUSIC_CELL_TYPE_COL"], "cell_type");
        assert_eq!(container.env["AUTONOMICS_MUSIC_SUBJECT_COL"], "subject_id");
        assert_eq!(
            container.env["AUTONOMICS_MUSIC_SELECT_CELL_TYPES"],
            "neuron,astrocyte"
        );
    }

    #[test]
    fn rejects_invalid_identifiers_and_parameters() {
        let mut value = spec();
        value.cell_type_col = "bad-name".into();
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.select_cell_types = vec!["neuron".into(), "neuron".into()];
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.nu = 0.0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.artifact_prefix = "relative".into();
        assert!(validate(&value).is_err());
    }
}
