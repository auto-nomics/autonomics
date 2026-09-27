//! Isolated script nodes. Python is reserved by Agent guidance for lightweight
//! DataFrame/File format conversion and interoperability bridging; substantive
//! analysis methods belong in dedicated DAG nodes.
//!
//! Rust materializes the upstream DataFrame as CSV, the pinned container reads
//! it as `input_table`, and user code must leave `output_table` in the global
//! environment. The same CSV is exposed as a File output after the run.

use std::collections::{BTreeMap, HashSet};
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
use crate::image_registry::registry_image;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptValueKind {
    Dataframe,
    File,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScriptInputSpec {
    /// Variable name exposed to user code. Must be a valid Python/R identifier.
    pub name: String,
    pub kind: ScriptValueKind,
    #[serde(default = "default_true")]
    pub required: bool,
    /// DataFrame staging format: csv, tsv, or parquet.
    #[serde(default = "default_csv_format")]
    pub format: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScriptOutputSpec {
    /// Variable name exposed to user code.
    pub name: String,
    pub kind: ScriptValueKind,
    /// Safe path relative to /work. Defaults to `<name>.<format>`.
    #[serde(default)]
    pub path: Option<String>,
    /// File/DataFrame format label.
    #[serde(default = "default_csv_format")]
    pub format: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScriptNodeSpec {
    pub code: String,
    /// Explicit generic input contract. Legacy mode is used when empty.
    #[serde(default)]
    pub inputs: Vec<ScriptInputSpec>,
    /// Explicit generic output contract. Empty selects legacy single-table mode.
    #[serde(default)]
    pub outputs: Vec<ScriptOutputSpec>,
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
fn default_true() -> bool {
    true
}
fn default_csv_format() -> String {
    "csv".into()
}

pub struct ScriptNode {
    ports: NodePorts,
    runtime: ScriptRuntime,
    spec: ScriptNodeSpec,
    generic: bool,
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
            generic: self.generic,
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

fn generic_port_layout(spec: &ScriptNodeSpec) -> Result<NodePorts, String> {
    let mut ports = NodePorts::new();
    for input in &spec.inputs {
        let data_type = match input.kind {
            ScriptValueKind::Dataframe => PortType::DataFrame,
            ScriptValueKind::File => PortType::File,
        };
        if input.required {
            ports = ports.add_input_port_of_type_with_label(None, data_type, input.name.clone());
        } else {
            ports = ports.add_optional_input_port_of_type(data_type);
        }
    }
    for output in &spec.outputs {
        let data_type = match output.kind {
            ScriptValueKind::Dataframe => PortType::DataFrame,
            ScriptValueKind::File => PortType::File,
        };
        ports = ports.add_output_port_of_type(None, data_type);
    }
    Ok(ports)
}

fn is_generic(spec: &ScriptNodeSpec) -> bool {
    !spec.outputs.is_empty()
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn valid_data_format(format: &str) -> bool {
    matches!(format, "csv" | "tsv" | "parquet")
}

fn validate_output_path(path: &str) -> Result<(), String> {
    let path = std::path::Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "script output path must be a safe relative /work path: `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn output_path(output: &ScriptOutputSpec) -> String {
    output
        .path
        .clone()
        .unwrap_or_else(|| format!("{}.{}", output.name, output.format.trim_start_matches('.')))
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
    if !spec.inputs.is_empty() && spec.outputs.is_empty() {
        return Err("outputs cannot be empty when inputs are specified".into());
    }
    let mut names = HashSet::new();
    for input in &spec.inputs {
        if !valid_identifier(&input.name) {
            return Err(format!(
                "input name `{}` must be a valid Python/R identifier",
                input.name
            ));
        }
        if !names.insert(input.name.clone()) {
            return Err(format!(
                "duplicate script input/output name `{}`",
                input.name
            ));
        }
        if input.kind == ScriptValueKind::Dataframe && !valid_data_format(&input.format) {
            return Err(format!(
                "input `{}` format must be csv, tsv, or parquet",
                input.name
            ));
        }
    }
    for output in &spec.outputs {
        if !valid_identifier(&output.name) {
            return Err(format!(
                "output name `{}` must be a valid Python/R identifier",
                output.name
            ));
        }
        if !names.insert(output.name.clone()) {
            return Err(format!(
                "duplicate script input/output name `{}`",
                output.name
            ));
        }
        if output.kind == ScriptValueKind::Dataframe && !valid_data_format(&output.format) {
            return Err(format!(
                "output `{}` format must be csv, tsv, or parquet",
                output.name
            ));
        }
        if output.format.trim().is_empty() {
            return Err(format!("output `{}` format cannot be empty", output.name));
        }
        validate_output_path(&output_path(output))?;
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

fn python_generic_wrapper(spec: &ScriptNodeSpec) -> String {
    let mut script = String::from(
        "import contextlib\nimport io\nimport os\nimport sys\nimport pandas as pd\n\nscript_inputs = {}\nscript_outputs = {}\n",
    );
    for (index, input) in spec.inputs.iter().enumerate() {
        let env = format!("AUTONOMICS_INPUT{index}");
        let path_expr = if input.required {
            format!("os.environ[{env:?}]")
        } else {
            format!("os.environ.get({env:?})")
        };
        script.push_str(&format!("{name}_path = {path_expr}\n", name = input.name));
        match input.kind {
            ScriptValueKind::Dataframe => {
                let reader = match input.format.as_str() {
                    "parquet" => format!("pd.read_parquet({name}_path)", name = input.name),
                    "tsv" => format!("pd.read_csv({name}_path, sep='\\t')", name = input.name),
                    _ => format!("pd.read_csv({name}_path)", name = input.name),
                };
                let optional = if input.required {
                    reader
                } else {
                    format!(
                        "None if {name}_path is None else {reader}",
                        name = input.name
                    )
                };
                script.push_str(&format!("{name} = {optional}\n", name = input.name));
            }
            ScriptValueKind::File => {
                script.push_str(&format!("{name} = {name}_path\n", name = input.name));
            }
        }
        script.push_str(&format!(
            "script_inputs[{name:?}] = {name}\n",
            name = input.name
        ));
    }
    for (index, output) in spec.outputs.iter().enumerate() {
        script.push_str(&format!(
            "{name}_path = os.environ[\"AUTONOMICS_OUTPUT{index}\"]\nscript_outputs[{name:?}] = {name}_path\nos.makedirs(os.path.dirname({name}_path) or \".\", exist_ok=True)\n",
            name = output.name
        ));
        if output.kind == ScriptValueKind::File {
            script.push_str(&format!("{name} = {name}_path\n", name = output.name));
        }
    }
    script.push_str(
        "user_stderr = io.StringIO()\nwith open(\"/work/.autonomics/files/user_code.py\", \"r\", encoding=\"utf-8\") as handle:\n    with contextlib.redirect_stderr(user_stderr):\n        exec(compile(handle.read(), \"user_code.py\", \"exec\"), globals())\nuser_stderr_text = user_stderr.getvalue()[-4000:]\nif user_stderr_text:\n    sys.stderr.write(user_stderr_text)\n",
    );
    for output in &spec.outputs {
        let path = serde_json::to_string(&output_path(output)).unwrap_or_else(|_| "\"\"".into());
        if output.kind == ScriptValueKind::Dataframe {
            let writer = match output.format.as_str() {
                "parquet" => "value.to_parquet(path, index=False)",
                "tsv" => "value.to_csv(path, index=False, sep='\\t')",
                _ => "value.to_csv(path, index=False)",
            };
            script.push_str(&format!(
                "if \"{name}\" in globals() and isinstance(globals()[\"{name}\"], pd.DataFrame) and not os.path.exists({name}_path):\n    value = globals()[\"{name}\"]\n    path = {name}_path\n    {writer}\n",
                name = output.name
            ));
        }
        script.push_str(&format!(
            "if not os.path.exists({name}_path) or os.path.getsize({name}_path) == 0:\n    message = \"script did not produce output \" + {path} + \"; write it to {name}_path\"\n    if user_stderr_text:\n        message += \"; user stderr:\\n\" + user_stderr_text\n    raise RuntimeError(message)\n",
            path = path,
            name = output.name,
        ));
    }
    script
}

fn r_generic_wrapper(spec: &ScriptNodeSpec) -> String {
    let mut script = String::from("script_inputs <- list()\nscript_outputs <- list()\n");
    for (index, input) in spec.inputs.iter().enumerate() {
        script.push_str(&format!(
            "{name}_path <- Sys.getenv(\"AUTONOMICS_INPUT{index}\", unset = \"\")\n",
            name = input.name
        ));
        match input.kind {
            ScriptValueKind::Dataframe => {
                let reader = match input.format.as_str() {
                    "parquet" => format!("arrow::read_parquet({name}_path)", name = input.name),
                    "tsv" => format!(
                        "utils::read.delim({name}_path, check.names = FALSE)",
                        name = input.name
                    ),
                    _ => format!(
                        "utils::read.csv({name}_path, check.names = FALSE)",
                        name = input.name
                    ),
                };
                if input.required {
                    script.push_str(&format!("{name} <- {reader}\n", name = input.name));
                } else {
                    script.push_str(&format!(
                        "if (nzchar({name}_path)) {{\n  {name} <- {reader}\n}} else {{\n  {name} <- NULL\n}}\n",
                        name = input.name
                    ));
                }
            }
            ScriptValueKind::File => {
                script.push_str(&format!(
                    "{name} <- if (nzchar({name}_path)) {name}_path else NULL\n",
                    name = input.name
                ));
            }
        }
        script.push_str(&format!(
            "script_inputs${name} <- {name}\n",
            name = input.name
        ));
    }
    for (index, output) in spec.outputs.iter().enumerate() {
        script.push_str(&format!(
            "{name}_path <- Sys.getenv(\"AUTONOMICS_OUTPUT{index}\")\nscript_outputs${name} <- {name}_path\ndir.create(dirname({name}_path), recursive = TRUE, showWarnings = FALSE)\n",
            name = output.name
        ));
    }
    script.push_str("source(\"/work/.autonomics/files/user_code.R\", local = globalenv())\n");
    for output in &spec.outputs {
        let path = output_path(output);
        if output.kind == ScriptValueKind::Dataframe {
            let writer = match output.format.as_str() {
                "parquet" => format!(
                    "arrow::write_parquet({name}, {name}_path)",
                    name = output.name
                ),
                "tsv" => format!(
                    "utils::write.table({name}, {name}_path, sep = \"\\t\", row.names = FALSE, quote = TRUE, qmethod = \"double\")",
                    name = output.name
                ),
                _ => format!(
                    "utils::write.table({name}, {name}_path, sep = \",\", row.names = FALSE, quote = TRUE, qmethod = \"double\")",
                    name = output.name
                ),
            };
            script.push_str(&format!(
                "if (exists(\"{name}\", envir = globalenv()) && inherits(get(\"{name}\", envir = globalenv()), \"data.frame\") && !file.exists({name}_path)) {{\n  {writer}\n}}\n",
                name = output.name
            ));
        }
        script.push_str(&format!(
            "if (!file.exists({name}_path) || file.info({name}_path)$size == 0) stop(\"script did not produce output `{path}`\")\n",
            name = output.name
        ));
    }
    script
}

pub fn container_spec(
    runtime: ScriptRuntime,
    spec: &ScriptNodeSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let image = match runtime {
        ScriptRuntime::Python => registry_image(
            SINGLE_CELL_WORKFLOW_IMAGE_REPOSITORY,
            SINGLE_CELL_WORKFLOW_IMAGE_DIGEST,
        )?,
        ScriptRuntime::R => {
            registry_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST)?
        }
    };
    let command = match runtime {
        ScriptRuntime::Python => vec!["python".into()],
        ScriptRuntime::R => vec!["Rscript".into()],
    };
    let generic = is_generic(spec);
    let script = match (runtime, generic) {
        (ScriptRuntime::Python, true) => python_generic_wrapper(spec),
        (ScriptRuntime::Python, false) => python_wrapper(),
        (ScriptRuntime::R, true) => r_generic_wrapper(spec),
        (ScriptRuntime::R, false) => r_wrapper(),
    };
    let user_file = match runtime {
        ScriptRuntime::Python => "user_code.py",
        ScriptRuntime::R => "user_code.R",
    };
    let outputs = if generic {
        spec.outputs
            .iter()
            .map(|output| ContainerCommandOutputSpec {
                path: output_path(output),
                format: Some(output.format.clone()),
            })
            .collect()
    } else {
        vec![ContainerCommandOutputSpec {
            path: "output.csv".into(),
            format: Some("csv".into()),
        }]
    };
    Ok(ContainerCommandSpec {
        image,
        command,
        script: Some(script),
        files: BTreeMap::from([(user_file.into(), spec.code.clone())]),
        env: Default::default(),
        outputs,
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
        gpus: None,
        user: None,
    })
}

fn temporary_input_path(spec: &ScriptNodeSpec, index: usize, format: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(&spec.code);
    digest.update(spec.artifact_prefix.as_bytes());
    digest.update(index.to_le_bytes());
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    format!(
        "/tmp/autonomics-script-input-{:x}-{}-{}.{}",
        digest.finalize(),
        std::process::id(),
        suffix,
        format
    )
}

fn script_file_format(format: &str) -> FileFormat {
    match format {
        "parquet" => FileFormat::Parquet,
        "tsv" => FileFormat::Tsv,
        _ => FileFormat::Csv,
    }
}

impl ScriptNode {
    async fn execute_generic(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mut container_inputs = Vec::with_capacity(self.spec.inputs.len());
        let mut temporary_inputs = Vec::new();
        for (index, input_spec) in self.spec.inputs.iter().enumerate() {
            let input = inputs.iter().find(|input| input.port as usize == index);
            let Some(input) = input else {
                if input_spec.required {
                    return Err(DagError::Schedule(format!(
                        "script input `{}` on port {index} is required",
                        input_spec.name
                    )));
                }
                continue;
            };
            match input_spec.kind {
                ScriptValueKind::File => {
                    container_inputs.push(NodeInput::file(index as u8, input.file_value()?.clone()))
                }
                ScriptValueKind::Dataframe => {
                    let path = temporary_input_path(&self.spec, index, &input_spec.format);
                    let frame = input.dataframe()?.clone();
                    let write_result = match input_spec.format.as_str() {
                        "parquet" => {
                            frame
                                .write_parquet(
                                    &path,
                                    DataFrameWriteOptions::new().with_single_file_output(true),
                                    None::<datafusion::config::TableParquetOptions>,
                                )
                                .await
                        }
                        "tsv" => {
                            let mut options = datafusion::config::CsvOptions::default();
                            options.delimiter = b'\t';
                            frame
                                .write_csv(
                                    &path,
                                    DataFrameWriteOptions::new().with_single_file_output(true),
                                    Some(options),
                                )
                                .await
                        }
                        _ => {
                            let mut options = datafusion::config::CsvOptions::default();
                            options.delimiter = b',';
                            frame
                                .write_csv(
                                    &path,
                                    DataFrameWriteOptions::new().with_single_file_output(true),
                                    Some(options),
                                )
                                .await
                        }
                    };
                    write_result.map_err(|error| {
                        DagError::Schedule(format!(
                            "cannot stage script input `{}`: {error}",
                            input_spec.name
                        ))
                    })?;
                    let file = FileRef::local(&path, Some(input_spec.format.clone())).map_err(
                        |error| {
                            DagError::Schedule(format!(
                                "cannot resolve staged script input `{}`: {error}",
                                input_spec.name
                            ))
                        },
                    )?;
                    temporary_inputs.push(path);
                    container_inputs.push(NodeInput::file(index as u8, file));
                }
            }
        }

        let container_outputs = match self.inner.execute(ctx, &container_inputs, reporter).await {
            Ok(outputs) => outputs,
            Err(error) => {
                for path in &temporary_inputs {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return Err(error);
            }
        };
        for path in &temporary_inputs {
            let _ = tokio::fs::remove_file(path).await;
        }

        let mut outputs = PortOutputs::new();
        for (index, output_spec) in self.spec.outputs.iter().enumerate() {
            let file = container_outputs
                .get(&(index as u8))
                .and_then(|value| value.as_file().ok())
                .cloned()
                .ok_or_else(|| {
                    DagError::Schedule(format!(
                        "script output `{}` on port {index} is missing",
                        output_spec.name
                    ))
                })?;
            match output_spec.kind {
                ScriptValueKind::File => {
                    outputs.insert_file(index as u8, file);
                }
                ScriptValueKind::Dataframe => {
                    let mut reader = FileToDataFrameNode::new_with_tabular_options(
                        Some(file.path.clone()),
                        Some(script_file_format(&output_spec.format)),
                        Vec::new(),
                        TabularReadOptions::default(),
                    );
                    let parsed = reader.execute(ctx, &[], reporter).await?;
                    let dataframe = parsed.dataframe(0)?.clone();
                    outputs.insert(index as u8, dataframe);
                }
            }
        }
        Ok(outputs)
    }
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
        if self.generic {
            return self.execute_generic(ctx, inputs, reporter).await;
        }
        let data_input = inputs
            .iter()
            .find(|input| input.port == 0)
            .ok_or_else(|| DagError::Schedule("script node requires a DataFrame input".into()))?;
        let frame = data_input.dataframe()?.clone();
        let path = temporary_input_path(&self.spec, 0, "csv");
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
            ScriptRuntime::Python => {
                "Runs inline Python in an isolated image only for DataFrame/File \
                 format conversion or interoperability bridging. Use a dedicated \
                 DAG node for actual analysis methods."
            }
            ScriptRuntime::R => "Runs inline R over DataFrame/File inputs in an isolated image.",
        }
    }

    fn doc(&self) -> &'static str {
        match self.runtime {
            ScriptRuntime::Python => {
                "**Scope**: Use `python_script` only for lightweight data-format \
        conversion or bridging between DataFrame/File representations. It is not \
        appropriate for applying actual analysis methods. Before using it, inspect \
        the registry and choose the dedicated DAG node for the required analysis; \
        if that node does not exist, report the analysis as unsupported rather than \
        reimplementing it in Python.\n\
\n\
        Legacy mode keeps input_table/file_input and output_table/CSV outputs. \
        Generic mode is selected by a non-empty outputs array: each input or output \
        spec creates a DAG port and exposes `<name>` plus `<name>_path` variables \
        to user code. DataFrame inputs are materialized as csv/tsv/parquet; \
        DataFrame outputs may be returned as `<name>` or written to \
        `<name>_path`; File outputs must be written to `<name>_path`. Packages are \
        preinstalled declarations only; runtime installation and network are disabled."
            }
            ScriptRuntime::R => {
                "Legacy mode keeps input_table/file_input and output_table/CSV outputs. \
        Generic mode is selected by a non-empty outputs array: each input or output \
        spec creates a DAG port and exposes `<name>` plus `<name>_path` variables \
        to user code. DataFrame inputs are materialized as csv/tsv/parquet; \
        DataFrame outputs may be returned as `<name>` or written to \
        `<name>_path`; File outputs must be written to `<name>_path`. Packages are \
        preinstalled declarations only; runtime installation and network are disabled."
            }
        }
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ScriptNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let parsed: ScriptNodeSpec = serde_json::from_value(spec)?;
        if is_generic(&parsed) {
            generic_port_layout(&parsed).map_err(dag_core::registry::error::Error::Unknown)
        } else {
            Ok(port_layout())
        }
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: ScriptNodeSpec = serde_json::from_value(spec)?;
        let generic = is_generic(&parsed);
        let ports = if generic {
            generic_port_layout(&parsed).map_err(dag_core::registry::error::Error::Unknown)?
        } else {
            port_layout()
        };
        let container = container_spec(self.runtime, &parsed)
            .map_err(dag_core::registry::error::Error::Unknown)?;
        let inner = ContainerCommandNode::new(
            self.kind(),
            container,
            Arc::clone(&self.runtime_connection),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(ScriptNode {
            ports,
            runtime: self.runtime,
            spec: parsed,
            generic,
            inner: Box::new(inner),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopConnection;

    #[async_trait::async_trait]
    impl container_runtime::PodmanConnection for NoopConnection {
        async fn run(
            &self,
            _request: container_runtime::ContainerRunRequest,
        ) -> Result<container_runtime::ContainerRunResult, container_runtime::ContainerRuntimeError>
        {
            Err(container_runtime::ContainerRuntimeError::Invalid(
                "metadata tests do not run containers".into(),
            ))
        }

        fn workspace_root(&self) -> &std::path::Path {
            std::path::Path::new("/tmp")
        }
    }

    #[test]
    fn python_metadata_limits_scope_to_format_bridging() {
        let factory = ScriptNodeFactory::python(
            std::sync::Arc::new(NoopConnection),
            std::sync::Arc::new(container_runtime::PanelCache::new("/tmp")),
        );

        assert!(factory.desc().contains("format conversion"));
        assert!(factory.desc().contains("dedicated DAG node"));
        assert!(
            factory
                .doc()
                .contains("not appropriate for applying actual analysis methods")
        );
    }

    #[test]
    fn builds_isolated_python_contract() {
        let spec = ScriptNodeSpec {
            code: "output_table = input_table".into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
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
            inputs: Vec::new(),
            outputs: Vec::new(),
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

    #[test]
    fn builds_generic_multi_io_contract() {
        let spec = ScriptNodeSpec {
            code: "output_table = input_table".into(),
            inputs: vec![
                ScriptInputSpec {
                    name: "input_table".into(),
                    kind: ScriptValueKind::Dataframe,
                    required: true,
                    format: "tsv".into(),
                },
                ScriptInputSpec {
                    name: "reference".into(),
                    kind: ScriptValueKind::File,
                    required: false,
                    format: "csv".into(),
                },
            ],
            outputs: vec![
                ScriptOutputSpec {
                    name: "output_table".into(),
                    kind: ScriptValueKind::Dataframe,
                    path: Some("results/output.parquet".into()),
                    format: "parquet".into(),
                },
                ScriptOutputSpec {
                    name: "artifact".into(),
                    kind: ScriptValueKind::File,
                    path: Some("results/artifact.txt".into()),
                    format: "txt".into(),
                },
            ],
            packages: Vec::new(),
            artifact_prefix: "/artifacts/generic-script-test".into(),
            timeout_s: 60,
            cpus: None,
            memory: None,
            pids_limit: None,
        };
        let ports = generic_port_layout(&spec).unwrap();
        assert_eq!(ports.input_ports().len(), 2);
        assert!(!ports.input_port(1).unwrap().required);
        assert_eq!(ports.output_ports().len(), 2);
        assert_eq!(ports.output_port(0).unwrap().data_type, PortType::DataFrame);
        assert_eq!(ports.output_port(1).unwrap().data_type, PortType::File);

        let container = container_spec(ScriptRuntime::Python, &spec).unwrap();
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.outputs[0].path, "results/output.parquet");
        assert_eq!(container.outputs[1].path, "results/artifact.txt");
        let wrapper = container.script.unwrap();
        assert!(wrapper.contains("script_inputs"));
        assert!(wrapper.contains("results/output.parquet"));
        assert!(wrapper.contains("artifact = artifact_path"));
        assert!(wrapper.contains("user stderr"));
    }

    #[test]
    fn python_generic_wrapper_autowrites_dataframes_with_defined_values() {
        let spec = ScriptNodeSpec {
            code: "table = pd.DataFrame({'id': [1]})".into(),
            inputs: Vec::new(),
            outputs: vec![
                ScriptOutputSpec {
                    name: "table".into(),
                    kind: ScriptValueKind::Dataframe,
                    path: None,
                    format: "csv".into(),
                },
                ScriptOutputSpec {
                    name: "table_tsv".into(),
                    kind: ScriptValueKind::Dataframe,
                    path: None,
                    format: "tsv".into(),
                },
                ScriptOutputSpec {
                    name: "table_parquet".into(),
                    kind: ScriptValueKind::Dataframe,
                    path: None,
                    format: "parquet".into(),
                },
            ],
            packages: Vec::new(),
            artifact_prefix: "/artifacts/python-script-test".into(),
            timeout_s: 60,
            cpus: None,
            memory: None,
            pids_limit: None,
        };

        let wrapper = python_generic_wrapper(&spec);
        assert!(wrapper.contains("value = globals()[\"table\"]"));
        assert!(wrapper.contains("path = table_path"));
        assert!(wrapper.contains("value.to_csv(path, index=False)"));
        assert!(wrapper.contains("value.to_csv(path, index=False, sep='\\t')"));
        assert!(wrapper.contains("value.to_parquet(path, index=False)"));
        assert!(!wrapper.contains("to_csv(value"));
        assert!(!wrapper.contains("to_parquet(value"));
    }
}
