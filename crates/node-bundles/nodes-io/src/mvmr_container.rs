//! Containerized multivariable MR node backed by the official R package.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use container_runtime::{K3sRuntime, PanelCache, PullPolicy};

pub const MVMR_CONTAINER_KIND: &str = "mvmr_container";
pub const MVMR_ORIGINAL_IMAGE: &str = "localhost/atc/mvmr:0.4.8";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/mvmr_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MvmrContainerSpec {
    pub beta_yg: String,
    pub sebeta_yg: String,
    pub beta_xg: Vec<String>,
    pub sebeta_xg: Vec<String>,
    #[serde(default)]
    pub label_column: Option<String>,
    #[serde(default)]
    pub gencov: f64,
    #[serde(default = "default_true")]
    pub strength: bool,
    #[serde(default = "default_true")]
    pub strhet: bool,
    #[serde(default = "default_true")]
    pub pleiotropy: bool,
    #[serde(default)]
    pub qhet: bool,
    #[serde(default)]
    pub pcor: Vec<Vec<f64>>,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct MvmrContainerNodeFactory {
    pub(crate) runtime: Arc<K3sRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl MvmrContainerNodeFactory {
    pub fn new(runtime: Arc<K3sRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MvmrContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for MvmrContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MvmrContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MVMR_CONTAINER_KIND
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

pub fn validate(spec: &MvmrContainerSpec) -> Result<(), String> {
    if spec.beta_yg.trim().is_empty() || spec.sebeta_yg.trim().is_empty() {
        return Err("beta_yg and sebeta_yg cannot be empty".into());
    }
    if spec.beta_xg.is_empty() || spec.beta_xg.len() != spec.sebeta_xg.len() {
        return Err("beta_xg and sebeta_xg must be non-empty and equal-length".into());
    }
    if !spec.gencov.is_finite() {
        return Err("gencov must be finite".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.qhet {
        let p = spec.beta_xg.len();
        if spec.pcor.len() != p
            || spec
                .pcor
                .iter()
                .any(|row| row.len() != p || row.iter().any(|value| !value.is_finite()))
        {
            return Err(format!("qhet requires a finite {p}x{p} pcor matrix"));
        }
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

fn r_pcor(matrix: &[Vec<f64>]) -> String {
    let values = matrix
        .iter()
        .flat_map(|row| row.iter())
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "matrix(c({values}), nrow = {n}, ncol = {n}, byrow = TRUE)",
        n = matrix.len()
    )
}

pub fn container_spec(spec: &MvmrContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let label = spec
        .label_column
        .as_deref()
        .map(r_string)
        .unwrap_or_else(|| "rownames(data)".into());
    let pcor = if spec.qhet {
        format!("pcor <- {}", r_pcor(&spec.pcor))
    } else {
        "pcor <- NULL".into()
    };

    let r_code = format!(
        "input <- Sys.getenv(\"AUTONOMICS_INPUT0\")\n\
         result_path <- Sys.getenv(\"AUTONOMICS_OUTPUT0\")\n\
         log_path <- Sys.getenv(\"AUTONOMICS_OUTPUT1\")\n\
         data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)\n\
         mvmr_input <- MVMR::format_mvmr(\n\
         \x20 BXGs = as.matrix(data[, {beta_xg}, drop = FALSE]),\n\
         \x20 BYG = data${beta_yg},\n\
         \x20 seBXGs = as.matrix(data[, {sebeta_xg}, drop = FALSE]),\n\
         \x20 seBYG = data${sebeta_yg},\n\
         \x20 RSID = {label}\n\
         )\n\
         {pcor}\n\
         gencov <- {gencov}\n\
         result <- list(input = mvmr_input, ivw = MVMR::ivw_mvmr(mvmr_input))\n\
         result$strength <- {strength_flag}\n\
         result$strhet <- {strhet_flag}\n\
         result$pleiotropy <- {pleiotropy_flag}\n\
         result$qhet <- {qhet_flag}\n\
         sink(log_path, split = TRUE)\n\
         print(result)\n\
         sink()\n\
         saveRDS(result, result_path)\n",
        beta_xg = r_strings(&spec.beta_xg),
        beta_yg = spec.beta_yg,
        sebeta_xg = r_strings(&spec.sebeta_xg),
        sebeta_yg = spec.sebeta_yg,
        label = label,
        pcor = pcor,
        strength_flag = if spec.strength {
            "MVMR::strength_mvmr(mvmr_input, gencov)".to_string()
        } else {
            "NULL".to_string()
        },
        strhet_flag = if spec.strhet {
            "MVMR::strhet_mvmr(mvmr_input, gencov)".to_string()
        } else {
            "NULL".to_string()
        },
        pleiotropy_flag = if spec.pleiotropy {
            "MVMR::pleiotropy_mvmr(mvmr_input, gencov)".to_string()
        } else {
            "NULL".to_string()
        },
        qhet_flag = if spec.qhet {
            "MVMR::qhet_mvmr(mvmr_input, pcor, CI = FALSE)".to_string()
        } else {
            "NULL".to_string()
        },
        gencov = spec.gencov,
    );

    Ok(ContainerCommandSpec {
        image: MVMR_ORIGINAL_IMAGE.into(),
        command: vec!["Rscript".into()],
        script: Some(r_code),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "mvmr.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "mvmr.log".into(),
                format: Some("mvmr_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Never,
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
}

impl NodeFactory for MvmrContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MVMR_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official MVMR in an ephemeral k3s Job."
    }

    fn doc(&self) -> &'static str {
        "Runs the official R MVMR package. Input is one tab-separated File with \
        outcome effect/SE and at least two exposure effect/SE columns. The \
        wrapper saves the official input and selected result objects as an RDS \
        artifact and emits their printed representation as a log artifact. No \
        numerical logic is reimplemented."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MvmrContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: MvmrContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::ContainerRuntime> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(MvmrContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MvmrContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MvmrContainerSpec {
        MvmrContainerSpec {
            beta_yg: "SBP_beta".into(),
            sebeta_yg: "SBP_se".into(),
            beta_xg: vec!["LDL_beta".into(), "HDL_beta".into()],
            sebeta_xg: vec!["LDL_se".into(), "HDL_se".into()],
            label_column: Some("SNP".into()),
            gencov: 0.0,
            strength: true,
            strhet: true,
            pleiotropy: true,
            qhet: false,
            pcor: Vec::new(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_mvmr_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(container.image, MVMR_ORIGINAL_IMAGE);
        assert!(container.panel_bundles.is_empty());
        assert!(
            container
                .script
                .as_deref()
                .unwrap()
                .contains("MVMR::format_mvmr")
        );
        assert!(container.script.as_deref().unwrap().contains("ivw_mvmr"));
        assert_eq!(container.outputs.len(), 2);
    }

    #[test]
    fn validates_qhet_matrix() {
        let mut value = spec();
        value.qhet = true;
        assert!(validate(&value).is_err());
        value.pcor = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        assert!(validate(&value).is_ok());
    }
}
