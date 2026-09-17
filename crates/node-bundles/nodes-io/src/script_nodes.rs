//! Isolated arbitrary Python/R DataFrame escape hatches.
//!
//! Rust materializes the upstream DataFrame as CSV, the pinned container reads
//! it as `input_table`, and user code must leave `output_table` in the global
//! environment. The same CSV is exposed as a File output after the run.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use datafusion::dataframe::DataFrameWriteOptions;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::file_to_dataframe::{
    FileFormat, FileToDataFrameNode, FileToDataFrameNodeFactory, TabularReadOptions,
};
use crate::image_registry::acr_image;
use crate::single_cell_h5ad::{
    SINGLE_CELL_WORKFLOW_IMAGE_DIGEST, SINGLE_CELL_WORKFLOW_IMAGE_REPOSITORY,
};
use crate::visualization_container::{VISUALIZATION_IMAGE_DIGEST, VISUALIZATION_IMAGE_REPOSITORY};
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};

pub const PYTHON_SCRIPT_KIND: &str = "python_script";
pub const R_SCRIPT_KIND: &str = "r_script";
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "4Gi";
const DEFAULT_PIDS_LIMIT: i64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptRuntime {
    Python,
    R,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScriptNodeSpec {
    pub code: String,
    /// Compatibility declaration for packages expected in the pinned image.
    /// Packages are not installed at runtime and network access is disabled.
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

fn default_artifact_prefix() -> String {
    "/artifacts/script".into()
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct ScriptNode {
    ports: NodePorts,
    runtime: ScriptRuntime,
    spec: ScriptNodeSpec,
    inner: Box<dyn DagNode>,
}

pub struct ScriptNodeFactory {
    runtime: ScriptRuntime,
    runtime_connection: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl ScriptNodeFactory {
    pub fn python(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime: ScriptRuntime::Python,
            runtime_connection: runtime,
            panel_cache,
        }
    }

    pub fn r(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime: ScriptRuntime::R,
            runtime_connection: runtime,
            panel_cache,
        }
    }
}

impl Clone for ScriptNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            runtime: self.runtime,
            spec: self.spec.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "input")
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port_of_type(None, PortType::DataFrame)
        .add_output_port_of_type(None, PortType::File)
}

fn validate(spec: &ScriptNodeSpec) -> Result<(), String> {
    if spec.code.trim().is_empty() {
        return Err("script code cannot be empty".into());
    }
    if spec.code.len() > MAX_SCRIPT_BYTES {
        return Err(format!("script code exceeds {MAX_SCRIPT_BYTES} bytes"));
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_s == 0 {
        return Err("timeout_s must be greater than zero".into());
    }
    if let Some(cpus) = spec.cpus
        && (!cpus.is_finite() || cpus <= 0.0)
    {
        return Err("cpus must be finite and greater than zero".into());
    }
    if let Some(limit) = spec.pids_limit
        && limit <= 0
    {
        return Err("pids_limit must be greater than zero".into());
    }
    if let Some(memory) = &spec.memory
        && memory.trim().is_empty()
    {
        return Err("memory cannot be empty".into());
    }
    if spec
        .packages
        .iter()
        .any(|package| package.trim().is_empty())
    {
        return Err("packages entries cannot be empty".into());
    }
    Ok(())
}

fn python_wrapper() -> String {
    r#"
import json
import os
import pandas as pd

input_path = os.environ["AUTONOMICS_INPUT0"]
file_input = os.environ.get("AUTONOMICS_INPUT1")
input_table = pd.read_csv(input_path)
with open("/work/.autonomics/files/user_code.py", "r", encoding="utf-8") as handle:
    exec(compile(handle.read(), "user_code.py", "exec"), globals())
if "output_table" not in globals():
    raise RuntimeError("script must define output_table")
output_table.to_csv(os.environ["AUTONOMICS_OUTPUT0"], index=False)
"#
    .into()
}

fn r_wrapper() -> String {
    r#"
input_path <- Sys.getenv("AUTONOMICS_INPUT0")
file_input <- Sys.getenv("AUTONOMICS_INPUT1")
input_table <- utils::read.csv(input_path, check.names = FALSE)
source("/work/.autonomics/files/user_code.R", local = globalenv())
if (!exists("output_table", envir = globalenv())) {
  stop("script must define output_table")
}
utils::write.table(
  output_table,
  Sys.getenv("AUTONOMICS_OUTPUT0"),
  sep = ",",
  row.names = FALSE,
  quote = TRUE,
  qmethod = "double"
)
"#
    .into()
}

pub fn container_spec(
    runtime: ScriptRuntime,
    spec: &ScriptNodeSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let image = match runtime {
        ScriptRuntime::Python => acr_image(
            SINGLE_CELL_WORKFLOW_IMAGE_REPOSITORY,
            SINGLE_CELL_WORKFLOW_IMAGE_DIGEST,
        )?,
        ScriptRuntime::R => acr_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST)?,
    };
    let command = match runtime {
        ScriptRuntime::Python => vec!["python".into()],
        ScriptRuntime::R => vec!["Rscript".into()],
    };
    let script = match runtime {
        ScriptRuntime::Python => python_wrapper(),
        ScriptRuntime::R => r_wrapper(),
    };
    let user_file = match runtime {
        ScriptRuntime::Python => "user_code.py",
        ScriptRuntime::R => "user_code.R",
    };
    Ok(ContainerCommandSpec {
        image,
        command,
        script: Some(script),
        files: std::collections::BTreeMap::from([(user_file.into(), spec.code.clone())]),
        env: Default::default(),
        outputs: vec![ContainerCommandOutputSpec {
            path: "output.csv".into(),
            format: Some("csv".into()),
        }],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_s,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: Some(spec.cpus.unwrap_or(DEFAULT_CPUS)),
        memory: Some(spec.memory.clone().unwrap_or_else(|| DEFAULT_MEMORY.into())),
        pids_limit: Some(spec.pids_limit.unwrap_or(DEFAULT_PIDS_LIMIT)),
        shm_size: None,
        user: None,
    })
}

fn temporary_input_path(spec: &ScriptNodeSpec) -> String {
    let mut digest = Sha256::new();
    digest.update(&spec.code);
    digest.update(spec.artifact_prefix.as_bytes());
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    format!(
        "/tmp/autonomics-script-input-{:x}-{}-{}.csv",
        digest.finalize(),
        std::process::id(),
        suffix
    )
}

#[async_trait]
impl DagNode for ScriptNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        match self.runtime {
            ScriptRuntime::Python => PYTHON_SCRIPT_KIND,
            ScriptRuntime::R => R_SCRIPT_KIND,
        }
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
        let data_input = inputs
            .iter()
            .find(|input| input.port == 0)
            .ok_or_else(|| DagError::Schedule("script node requires a DataFrame input".into()))?;
        let frame = data_input.dataframe()?.clone();
        let path = temporary_input_path(&self.spec);
        frame
            .write_csv(
                &path,
                DataFrameWriteOptions::new().with_single_file_output(true),
                None,
            )
            .await
            .map_err(|error| DagError::Schedule(format!("cannot stage script input: {error}")))?;
        let input_file = FileRef::local(&path, Some("csv".into())).map_err(|error| {
            DagError::Schedule(format!("cannot resolve staged script input: {error}"))
        })?;
        let mut container_inputs = vec![NodeInput::file(0, input_file)];
        if let Some(file_input) = inputs.iter().find(|input| input.port == 1) {
            container_inputs.push(file_input.clone());
        }
        let container_outputs = match self.inner.execute(ctx, &container_inputs, reporter).await {
            Ok(outputs) => outputs,
            Err(error) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(error);
            }
        };
        let _ = tokio::fs::remove_file(&path).await;
        let output_file = container_outputs
            .get(&0)
            .and_then(|value| value.as_file().ok())
            .cloned()
            .ok_or_else(|| DagError::Schedule("script node produced no CSV output".into()))?;
        let mut reader = FileToDataFrameNode::new_with_tabular_options(
            Some(output_file.path.clone()),
            Some(FileFormat::Csv),
            Vec::new(),
            TabularReadOptions::default(),
        );
        let mut outputs = reader.execute(ctx, &[], reporter).await?;
        outputs.insert_file(1, output_file);
        let _ = tokio::fs::remove_file(&path).await;
        Ok(outputs)
    }
}

impl NodeFactory for ScriptNodeFactory {
    fn kind(&self) -> &'static str {
        match self.runtime {
            ScriptRuntime::Python => PYTHON_SCRIPT_KIND,
            ScriptRuntime::R => R_SCRIPT_KIND,
        }
    }

    fn desc(&self) -> &'static str {
        match self.runtime {
            ScriptRuntime::Python => "Runs inline Python over one DataFrame in an isolated image.",
            ScriptRuntime::R => "Runs inline R over one DataFrame in an isolated image.",
        }
    }

    fn doc(&self) -> &'static str {
        "Input port 0 becomes `input_table`; optional input port 1 is exposed as \
        a read-only file path variable (`file_input`). User code must define \
        `output_table`. Output ports are the parsed DataFrame and its CSV File. \
        Packages are preinstalled declarations only; runtime package installation \
        and network access are disabled."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ScriptNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: ScriptNodeSpec = serde_json::from_value(spec)?;
        let container = container_spec(self.runtime, &parsed)
            .map_err(dag_core::registry::error::Error::Unknown)?;
        let inner = ContainerCommandNode::new(
            container,
            Arc::clone(&self.runtime_connection),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(ScriptNode {
            ports: port_layout(),
            runtime: self.runtime,
            spec: parsed,
            inner: Box::new(inner),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_isolated_python_contract() {
        let spec = ScriptNodeSpec {
            code: "output_table = input_table".into(),
            packages: vec!["pandas".into()],
            artifact_prefix: "/artifacts/python-script-test".into(),
            timeout_s: 60,
            cpus: None,
            memory: None,
            pids_limit: None,
        };
        let container = container_spec(ScriptRuntime::Python, &spec).unwrap();
        assert_eq!(container.command.as_slice(), ["python"]);
        assert_eq!(container.network, "isolated");
        assert!(container.files.contains_key("user_code.py"));
    }

    #[test]
    fn builds_isolated_r_contract() {
        let spec = ScriptNodeSpec {
            code: "output_table <- input_table".into(),
            packages: Vec::new(),
            artifact_prefix: "/artifacts/r-script-test".into(),
            timeout_s: 60,
            cpus: None,
            memory: None,
            pids_limit: None,
        };
        let container = container_spec(ScriptRuntime::R, &spec).unwrap();
        assert_eq!(container.command.as_slice(), ["Rscript"]);
        assert!(container.files.contains_key("user_code.R"));
        assert!(container.read_only_rootfs);
    }
}
