//! Containerized DESeq2 differential-expression node.
//!
//! The wrapper owns the two-file analysis contract and delegates all statistical
//! computation to the pinned official DESeq2 image. It deliberately does not
//! build an R formula from arbitrary user input.

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

pub const DESEQ2_DE_CONTAINER_KIND: &str = "deseq2_de_container";
pub const DESEQ2_IMAGE_REPOSITORY: &str = "deseq2";
pub const DESEQ2_IMAGE_DIGEST: &str =
    "sha256:8b2e2a78d87293e6cae6dbed2e283dff1dd8461a7f993cb347ca9810698b2b3e";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/deseq2_de_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "4Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;
const DEFAULT_SHM_SIZE: &str = "1Gi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Deseq2FitType {
    Parametric,
    Local,
    Mean,
}

impl Deseq2FitType {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Parametric => "parametric",
            Self::Local => "local",
            Self::Mean => "mean",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Deseq2DeContainerSpec {
    pub condition_reference: String,
    pub condition_test: String,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default = "default_fit_type")]
    pub fit_type: Deseq2FitType,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_threads")]
    pub threads: u8,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_fit_type() -> Deseq2FitType {
    Deseq2FitType::Parametric
}

fn default_alpha() -> f64 {
    0.1
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

pub struct Deseq2DeContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl Deseq2DeContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct Deseq2DeContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for Deseq2DeContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for Deseq2DeContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        DESEQ2_DE_CONTAINER_KIND
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

pub fn validate(spec: &Deseq2DeContainerSpec) -> Result<(), String> {
    if spec.condition_reference.trim().is_empty() {
        return Err("condition_reference cannot be empty".into());
    }
    if spec.condition_test.trim().is_empty() {
        return Err("condition_test cannot be empty".into());
    }
    if spec.condition_reference == spec.condition_test {
        return Err("condition_test and condition_reference must differ".into());
    }
    if any_duplicate(&spec.covariates) {
        return Err("covariates cannot contain duplicates".into());
    }
    for covariate in &spec.covariates {
        if covariate == "sample_id" || covariate == "condition" {
            return Err("sample_id and condition cannot be listed as covariates".into());
        }
        if !is_simple_r_identifier(covariate) {
            return Err(format!(
                "covariate `{covariate}` must be a simple R identifier"
            ));
        }
    }
    if !spec.alpha.is_finite() || spec.alpha <= 0.0 || spec.alpha >= 1.0 {
        return Err("alpha must be finite and lie strictly between 0 and 1".into());
    }
    if spec.threads != 1 {
        return Err("threads must be 1 in the deterministic first contract".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn any_duplicate(values: &[String]) -> bool {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted.windows(2).any(|window| window[0] == window[1])
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

pub fn container_spec(spec: &Deseq2DeContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_DESEQ2_CONDITION_REFERENCE".into(),
            spec.condition_reference.clone(),
        ),
        (
            "AUTONOMICS_DESEQ2_CONDITION_TEST".into(),
            spec.condition_test.clone(),
        ),
        (
            "AUTONOMICS_DESEQ2_COVARIATES".into(),
            spec.covariates.join(","),
        ),
        (
            "AUTONOMICS_DESEQ2_FIT_TYPE".into(),
            spec.fit_type.as_label().into(),
        ),
        ("AUTONOMICS_DESEQ2_ALPHA".into(), spec.alpha.to_string()),
        ("AUTONOMICS_DESEQ2_THREADS".into(), spec.threads.to_string()),
    ]);

    Ok(ContainerCommandSpec {
        image: acr_image(DESEQ2_IMAGE_REPOSITORY, DESEQ2_IMAGE_DIGEST)?,
        command: vec![
            "Rscript".into(),
            "--vanilla".into(),
            "/opt/autonomics/deseq2_runner.R".into(),
        ],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "results.tsv".into(),
                format: Some("deseq2_results_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "normalized_counts.tsv".into(),
                format: Some("deseq2_normalized_counts_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "size_factors.tsv".into(),
                format: Some("deseq2_size_factors_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "deseq2_dataset.rds".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "run_report.json".into(),
                format: Some("deseq2_run_report_json".into()),
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

impl NodeFactory for Deseq2DeContainerNodeFactory {
    fn kind(&self) -> &'static str {
        DESEQ2_DE_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official DESeq2 differential expression in an isolated container."
    }

    fn doc(&self) -> &'static str {
        "Input ports are a raw integer count matrix and sample metadata. The \
        node constructs `~ <covariates> + condition`, estimates and tests the \
        negative-binomial model with official DESeq2, and publishes the full \
        result table, normalized counts, size factors, fitted DESeqDataSet, \
        and a run report. No reference panel is required."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(Deseq2DeContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: Deseq2DeContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(Deseq2DeContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: Deseq2DeContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Deseq2DeContainerSpec {
        Deseq2DeContainerSpec {
            condition_reference: "untreated".into(),
            condition_test: "treated".into(),
            covariates: vec!["type".into()],
            fit_type: default_fit_type(),
            alpha: default_alpha(),
            threads: default_threads(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_deseq2_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(DESEQ2_IMAGE_REPOSITORY, DESEQ2_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(
            container.command,
            vec![
                "Rscript".to_string(),
                "--vanilla".to_string(),
                "/opt/autonomics/deseq2_runner.R".to_string()
            ]
        );
        assert!(container.script.is_none());
        assert!(container.panel_bundles.is_empty());
        assert!(container.panels.is_empty());
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.outputs.len(), 5);
        assert_eq!(container.env["AUTONOMICS_DESEQ2_COVARIATES"], "type");
        assert_eq!(container.env["AUTONOMICS_DESEQ2_FIT_TYPE"], "parametric");
        assert_eq!(container.env["AUTONOMICS_DESEQ2_ALPHA"], "0.1");
    }

    #[test]
    fn rejects_reserved_or_unsafe_covariates() {
        let mut value = spec();
        value.covariates = vec!["condition".into()];
        assert!(validate(&value).is_err());

        value.covariates = vec!["bad-name".into()];
        assert!(validate(&value).is_err());

        value.covariates = vec!["batch".into(), "batch".into()];
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_invalid_statistical_parameters() {
        let mut value = spec();
        value.condition_reference = "same".into();
        value.condition_test = "same".into();
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.alpha = 1.0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.threads = 2;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_relative_artifact_prefix() {
        let mut value = spec();
        value.artifact_prefix = "artifacts/deseq2".into();
        assert!(validate(&value).is_err());
    }
}
