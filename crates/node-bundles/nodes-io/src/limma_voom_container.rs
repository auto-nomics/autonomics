//! Containerized limma + voom + eBayes differential expression wrapper.
//!
//! Owns the two-file contract (count matrix + sample metadata) and delegates
//! statistical computation to the pinned official limma/edgeR image. Supports
//! both categorical and continuous outcomes, TMM or quantile normalization,
//! optional covariates, and writes the top-table along with voom weights and
//! the normalized expression matrix.

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
use crate::image_registry::registry_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const LIMMA_VOOM_CONTAINER_KIND: &str = "limma_voom_container";
pub const LIMMA_VOOM_IMAGE_REPOSITORY: &str = "bulk-rnaseq";
pub const LIMMA_VOOM_IMAGE_DIGEST: &str =
    "sha256:c63fb113760b97bad1b1761d1daf4ad3241aa32e054ab20dd3a6bced4a552a6d";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/limma_voom_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "6Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;
const DEFAULT_SHM_SIZE: &str = "1Gi";

fn default_outcome_mode() -> String {
    "categorical".into()
}

fn default_normalization() -> String {
    "tmm".into()
}

fn default_alpha() -> f64 {
    0.05
}

fn default_threads() -> u8 {
    1
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LimmaVoomContainerSpec {
    pub outcome_var: String,
    pub contrast_var: String,
    #[serde(default)]
    pub contrast_levels: Vec<String>,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default = "default_outcome_mode")]
    pub outcome_mode: String,
    #[serde(default = "default_normalization")]
    pub normalization: String,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_threads")]
    pub threads: u8,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

pub struct LimmaVoomContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LimmaVoomContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LimmaVoomContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for LimmaVoomContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for LimmaVoomContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LIMMA_VOOM_CONTAINER_KIND
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

pub fn validate(spec: &LimmaVoomContainerSpec) -> Result<(), String> {
    if spec.outcome_var.trim().is_empty() {
        return Err("outcome_var cannot be empty".into());
    }
    if spec.contrast_var.trim().is_empty() {
        return Err("contrast_var cannot be empty".into());
    }
    if !spec.outcome_mode.is_empty()
        && !["categorical", "continuous"].contains(&spec.outcome_mode.as_str())
    {
        return Err("outcome_mode must be categorical or continuous".into());
    }
    if !["tmm", "quantile", "none"].contains(&spec.normalization.as_str()) {
        return Err("normalization must be tmm, quantile, or none".into());
    }
    if spec.outcome_mode == "continuous" && !spec.contrast_levels.is_empty() {
        return Err("contrast_levels only applies to categorical outcome_mode".into());
    }
    if spec.outcome_mode == "categorical" && spec.contrast_levels.len() != 2 {
        return Err(
            "contrast_levels must contain exactly two entries for categorical outcome".into(),
        );
    }
    if spec.contrast_var != spec.outcome_var {
        return Err(
            "contrast_var must equal outcome_var in the deterministic first contract".into(),
        );
    }
    if spec.alpha <= 0.0 || spec.alpha >= 1.0 || !spec.alpha.is_finite() {
        return Err("alpha must lie strictly between 0 and 1".into());
    }
    if spec.threads != 1 {
        return Err("threads must be 1 in the deterministic first contract".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    let mut sorted_cov = spec.covariates.clone();
    sorted_cov.sort();
    if sorted_cov.windows(2).any(|w| w[0] == w[1]) {
        return Err("covariates cannot contain duplicates".into());
    }
    for cov in &spec.covariates {
        if cov == &spec.outcome_var || cov == "sample_id" {
            return Err(format!("covariate `{cov}` is reserved"));
        }
        if !is_simple_r_identifier(cov) {
            return Err(format!("covariate `{cov}` must be a simple R identifier"));
        }
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

pub fn container_spec(spec: &LimmaVoomContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_LIMMA_OUTCOME".to_string(),
            spec.outcome_var.clone(),
        ),
        (
            "AUTONOMICS_LIMMA_CONTRAST_VAR".to_string(),
            spec.contrast_var.clone(),
        ),
        (
            "AUTONOMICS_LIMMA_CONTRAST_LEVELS".to_string(),
            spec.contrast_levels.join(","),
        ),
        (
            "AUTONOMICS_LIMMA_COVARIATES".to_string(),
            spec.covariates.join(","),
        ),
        (
            "AUTONOMICS_LIMMA_OUTCOME_MODE".to_string(),
            spec.outcome_mode.clone(),
        ),
        (
            "AUTONOMICS_LIMMA_NORMALIZATION".to_string(),
            spec.normalization.clone(),
        ),
        ("AUTONOMICS_LIMMA_ALPHA".to_string(), spec.alpha.to_string()),
        (
            "AUTONOMICS_LIMMA_THREADS".to_string(),
            spec.threads.to_string(),
        ),
    ]);

    Ok(ContainerCommandSpec {
        image: registry_image(LIMMA_VOOM_IMAGE_REPOSITORY, LIMMA_VOOM_IMAGE_DIGEST)?,
        command: vec![
            "Rscript".to_string(),
            "--vanilla".to_string(),
            "/opt/autonomics/limma_voom_runner.R".to_string(),
        ],
        script: None,
        env,
        files: BTreeMap::new(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "results.tsv".into(),
                format: Some("limma_voom_results_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "voom_weights.tsv".into(),
                format: Some("limma_voom_weights_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "normalized_expression.tsv".into(),
                format: Some("limma_voom_normalized_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "contrast_summary.tsv".into(),
                format: Some("limma_voom_contrast_summary_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "run_report.json".into(),
                format: Some("limma_voom_run_report_json".into()),
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
        .add_input_port_of_type_with_label(None, PortType::File, "count_matrix")
        .add_input_port_of_type_with_label(None, PortType::File, "sample_metadata")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for LimmaVoomContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LIMMA_VOOM_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official limma + voom + eBayes in an isolated container."
    }

    fn doc(&self) -> &'static str {
        "Input ports are an integer count matrix and sample metadata. The \
        node constructs the requested design formula, applies TMM (or quantile) \
        normalization, fits limma::voom + eBayes, optionally contrasts two \
        outcome levels, and publishes the top table, voom weights, normalized \
        expression, contrast summary and a run report. No reference panel is \
        required."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LimmaVoomContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LimmaVoomContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            self.kind(),
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(LimmaVoomContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LimmaVoomContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LimmaVoomContainerSpec {
        LimmaVoomContainerSpec {
            outcome_var: "condition".into(),
            contrast_var: "condition".into(),
            contrast_levels: vec!["ctrl".into(), "case".into()],
            covariates: vec!["batch".into()],
            outcome_mode: default_outcome_mode(),
            normalization: default_normalization(),
            alpha: default_alpha(),
            threads: default_threads(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_limma_voom_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            registry_image(LIMMA_VOOM_IMAGE_REPOSITORY, LIMMA_VOOM_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(
            container.command,
            vec![
                "Rscript".to_string(),
                "--vanilla".to_string(),
                "/opt/autonomics/limma_voom_runner.R".to_string()
            ]
        );
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.outputs.len(), 5);
        assert_eq!(container.env["AUTONOMICS_LIMMA_OUTCOME"], "condition");
        assert_eq!(container.env["AUTONOMICS_LIMMA_NORMALIZATION"], "tmm");
        assert_eq!(container.env["AUTONOMICS_LIMMA_ALPHA"], "0.05");
    }

    #[test]
    fn rejects_continuous_with_contrast_levels() {
        let mut value = spec();
        value.contrast_var = "duration".into();
        value.outcome_var = "duration".into();
        value.outcome_mode = "continuous".into();
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_reserved_covariates() {
        let mut value = spec();
        value.covariates = vec!["sample_id".into()];
        assert!(validate(&value).is_err());
        value.covariates = vec!["bad-name".into()];
        assert!(validate(&value).is_err());
        value.covariates = vec!["batch".into(), "batch".into()];
        assert!(validate(&value).is_err());
    }
}
