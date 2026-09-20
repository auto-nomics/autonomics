//! Containerized preranked GSEA backed by the Bioconductor fgsea package.

use std::sync::Arc;

use arrow::csv::{ReaderBuilder, WriterBuilder, reader::Format};
use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const PATHWAY_GSEA_CONTAINER_KIND: &str = "pathway_gsea_container";
pub const PATHWAY_GSEA_IMAGE_REPOSITORY: &str = "pathway-gsea";
pub const PATHWAY_GSEA_IMAGE_DIGEST: &str =
    "sha256:1eb3a32abe2f910e8a3e2c7b5cd8d9f45bf267dc7c57605103e5c30310740275";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/pathway_gsea_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "8Gi";
const DEFAULT_PIDS_LIMIT: i64 = 512;
const DEFAULT_SHM_SIZE: &str = "1Gi";

fn default_gene_col() -> String {
    "gene".into()
}

fn default_score_col() -> String {
    "score".into()
}

fn default_pathway_col() -> String {
    "pathway_name".into()
}

fn default_pathway_gene_col() -> String {
    "gene".into()
}

fn default_min_size() -> usize {
    15
}

fn default_max_size() -> usize {
    500
}

fn default_eps() -> f64 {
    1e-50
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct PathwayGseaContainerSpec {
    #[serde(default = "default_gene_col")]
    pub gene_col: String,
    #[serde(default = "default_score_col")]
    pub score_col: String,
    #[serde(default = "default_pathway_col")]
    pub pathway_col: String,
    #[serde(default = "default_pathway_gene_col")]
    pub pathway_gene_col: String,
    #[serde(default = "default_min_size")]
    pub min_size: usize,
    #[serde(default = "default_max_size")]
    pub max_size: usize,
    #[serde(default = "default_eps")]
    pub eps: f64,
    /// VFS prefix used to publish the TSV report and JSON artifact.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PathwayGseaError {
    #[error("invalid pathway_gsea_container spec: {0}")]
    Invalid(String),
    #[error("pathway_gsea_container failed: {0}")]
    Runtime(String),
}

impl From<PathwayGseaError> for DagError {
    fn from(error: PathwayGseaError) -> Self {
        DagError::Schedule(error.to_string())
    }
}

pub struct PathwayGseaContainerNodeFactory {
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl PathwayGseaContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct PathwayGseaContainerNode {
    ports: NodePorts,
    spec: PathwayGseaContainerSpec,
    inner: Box<dyn DagNode>,
}

impl Clone for PathwayGseaContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            spec: self.spec.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

pub fn validate(spec: &PathwayGseaContainerSpec) -> Result<(), String> {
    for (name, value) in [
        ("gene_col", &spec.gene_col),
        ("score_col", &spec.score_col),
        ("pathway_col", &spec.pathway_col),
        ("pathway_gene_col", &spec.pathway_gene_col),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
    }
    if spec.min_size == 0 {
        return Err("min_size must be greater than zero".into());
    }
    if spec.max_size < spec.min_size {
        return Err("max_size must be at least min_size".into());
    }
    if !spec.eps.is_finite() || spec.eps <= 0.0 {
        return Err("eps must be finite and greater than zero".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if let Some(cpus) = spec.cpus
        && (!cpus.is_finite() || cpus <= 0.0)
    {
        return Err("cpus must be finite and greater than zero".into());
    }
    if let Some(memory) = &spec.memory
        && memory.trim().is_empty()
    {
        return Err("memory cannot be empty".into());
    }
    Ok(())
}

const R_SCRIPT: &str = r###"
gene_col <- Sys.getenv("AUTONOMICS_GENE_COL")
score_col <- Sys.getenv("AUTONOMICS_SCORE_COL")
pathway_col <- Sys.getenv("AUTONOMICS_PATHWAY_COL")
pathway_gene_col <- Sys.getenv("AUTONOMICS_PATHWAY_GENE_COL")
min_size <- as.integer(Sys.getenv("AUTONOMICS_MIN_SIZE"))
max_size <- as.integer(Sys.getenv("AUTONOMICS_MAX_SIZE"))
eps <- as.numeric(Sys.getenv("AUTONOMICS_EPS"))

rank_table <- data.table::fread(Sys.getenv("AUTONOMICS_INPUT0"), check.names = FALSE)
if (!gene_col %in% names(rank_table)) stop("rank table has no column: ", gene_col)
if (!score_col %in% names(rank_table)) stop("rank table has no column: ", score_col)
genes <- as.character(rank_table[[gene_col]])
if (anyNA(genes) || any(!nzchar(genes))) stop("rank gene column contains null or empty values")
if (anyDuplicated(genes)) stop("rank gene column contains duplicate values")
scores <- as.numeric(rank_table[[score_col]])
if (anyNA(scores)) stop("rank score column contains null or nonnumeric values")
stats <- setNames(scores, genes)

if (grepl("\\.gmt$", Sys.getenv("AUTONOMICS_INPUT1"))) {
  gmt_lines <- readLines(Sys.getenv("AUTONOMICS_INPUT1"), warn = FALSE)
  gmt_lines <- gmt_lines[nzchar(gmt_lines)]
  if (!length(gmt_lines)) stop("GMT file contains no gene sets")
  fields <- strsplit(gmt_lines, "\t", fixed = TRUE)
  if (any(lengths(fields) < 3L)) stop("GMT contains a set without genes")
  pathways <- lapply(fields, function(field) field[-c(1, 2)])
  names(pathways) <- vapply(fields, `[`, character(1), 1L)
  if (anyDuplicated(names(pathways))) stop("GMT contains duplicate pathway names")
} else {
  pathway_table <- data.table::fread(Sys.getenv("AUTONOMICS_INPUT1"), check.names = FALSE)
  if (!pathway_col %in% names(pathway_table)) stop("pathway table has no column: ", pathway_col)
  if (!pathway_gene_col %in% names(pathway_table)) stop("pathway table has no column: ", pathway_gene_col)
  set_names <- as.character(pathway_table[[pathway_col]])
  set_genes <- as.character(pathway_table[[pathway_gene_col]])
  if (anyNA(set_names) || any(!nzchar(set_names))) stop("pathway column contains null or empty values")
  if (anyNA(set_genes) || any(!nzchar(set_genes))) stop("pathway gene column contains null or empty values")
  pathways <- split(set_genes, set_names)
}
pathways <- lapply(pathways, unique)

result <- fgsea::fgseaMultilevel(
  pathways = pathways,
  stats = stats,
  minSize = min_size,
  maxSize = max_size,
  eps = eps
)
result$leadingEdge <- vapply(result$leadingEdge, paste, character(1), collapse = ";")
report <- result[, .(pathway, pval, padj, NES, size, leadingEdge)]
data.table::fwrite(report, Sys.getenv("AUTONOMICS_OUTPUT0"), sep = "\t", quote = FALSE)

json <- list(
  method = "fgseaMultilevel",
  parameters = list(
    min_size = min_size,
    max_size = max_size,
    eps = eps,
    gene_col = gene_col,
    score_col = score_col,
    pathway_col = pathway_col,
    pathway_gene_col = pathway_gene_col
  ),
  results = report
)
writeLines(
  jsonlite::toJSON(json, auto_unbox = TRUE, pretty = TRUE, na = "null"),
  Sys.getenv("AUTONOMICS_OUTPUT1")
)
"###;

pub fn container_spec(spec: &PathwayGseaContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = [
        ("AUTONOMICS_GENE_COL", spec.gene_col.clone()),
        ("AUTONOMICS_SCORE_COL", spec.score_col.clone()),
        ("AUTONOMICS_PATHWAY_COL", spec.pathway_col.clone()),
        ("AUTONOMICS_PATHWAY_GENE_COL", spec.pathway_gene_col.clone()),
        ("AUTONOMICS_MIN_SIZE", spec.min_size.to_string()),
        ("AUTONOMICS_MAX_SIZE", spec.max_size.to_string()),
        ("AUTONOMICS_EPS", spec.eps.to_string()),
    ];
    Ok(ContainerCommandSpec {
        image: acr_image(PATHWAY_GSEA_IMAGE_REPOSITORY, PATHWAY_GSEA_IMAGE_DIGEST)?,
        command: vec!["Rscript".into()],
        script: Some(R_SCRIPT.into()),
        files: Default::default(),
        env: env
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "fgsea_report.tsv".into(),
                format: Some("fgsea_report_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "fgsea_report.json".into(),
                format: Some("fgsea_report_json".into()),
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
        cpus: Some(spec.cpus.unwrap_or(DEFAULT_CPUS)),
        memory: Some(spec.memory.clone().unwrap_or_else(|| DEFAULT_MEMORY.into())),
        pids_limit: Some(DEFAULT_PIDS_LIMIT),
        shm_size: Some(DEFAULT_SHM_SIZE.into()),
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::Any)
        .add_optional_input_port_of_type(PortType::Any)
        .add_output_port(Some(report_schema()))
        .add_output_port_of_type(None, PortType::File)
}

fn report_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pathway", DataType::Utf8, false),
        Field::new("pval", DataType::Float64, true),
        Field::new("padj", DataType::Float64, true),
        Field::new("NES", DataType::Float64, true),
        Field::new("size", DataType::Int32, true),
        Field::new("leadingEdge", DataType::Utf8, true),
    ]))
}

fn input_file(inputs: &[NodeInput], port: u8) -> Option<&FileRef> {
    inputs
        .iter()
        .find(|input| input.port == port)
        .and_then(|input| input.file_value().ok())
}

fn input_dataframe(inputs: &[NodeInput], port: u8) -> Option<&DataFrame> {
    inputs
        .iter()
        .find(|input| input.port == port)
        .and_then(|input| input.dataframe().ok())
}

fn staging_prefix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "{DEFAULT_ARTIFACT_PREFIX}/staging/{}-{nanos}",
        std::process::id()
    )
}

fn require_storage(ctx: &NodeCtx) -> Result<Arc<vfs::OpendalFileStorage>, PathwayGseaError> {
    ctx.opendal
        .clone()
        .ok_or_else(|| PathwayGseaError::Runtime("pathway_gsea requires a runtime VFS".into()))
}

fn storage_virtual_path(path: &str) -> Option<String> {
    path.strip_prefix("vfs://")
        .map(str::to_string)
        .or_else(|| path.starts_with('/').then(|| path.to_string()))
}

fn file_extension(path: &str) -> String {
    storage_virtual_path(path)
        .unwrap_or_else(|| path.to_string())
        .strip_prefix("file://")
        .unwrap_or(&storage_virtual_path(path).unwrap_or_else(|| path.to_string()))
        .to_ascii_lowercase()
}

async fn stage_dataframe(
    ctx: &NodeCtx,
    df: &DataFrame,
    path: &str,
) -> Result<(), PathwayGseaError> {
    let batches = df
        .clone()
        .collect()
        .await
        .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?;
    if batches.iter().map(RecordBatch::num_rows).sum::<usize>() == 0 {
        return Err(PathwayGseaError::Invalid(
            "DataFrame input contains no rows".into(),
        ));
    }

    let mut tsv = Vec::new();
    let mut tsv_writer = WriterBuilder::new()
        .with_header(true)
        .with_delimiter(b'\t')
        .build(&mut tsv);
    for batch in &batches {
        tsv_writer
            .write(batch)
            .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?;
    }
    drop(tsv_writer);

    let storage = require_storage(ctx)?;
    storage
        .write_bytes(path, tsv)
        .await
        .map_err(|error| PathwayGseaError::Runtime(error.to_string()))
}

async fn report_to_dataframe(
    ctx: &NodeCtx,
    report: &FileRef,
) -> Result<DataFrame, PathwayGseaError> {
    let storage = require_storage(ctx)?;
    let virtual_path = storage_virtual_path(&report.path).ok_or_else(|| {
        PathwayGseaError::Runtime(format!(
            "container report `{}` is not addressable through the runtime VFS",
            report.path
        ))
    })?;
    let bytes = storage
        .resolve(&virtual_path)
        .read(&storage.resolve_path(&virtual_path))
        .await
        .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?;
    let reader = ReaderBuilder::new(report_schema())
        .with_header(true)
        .with_delimiter(b'\t')
        .with_format(Format::default().with_header(true).with_delimiter(b'\t'))
        .build(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.map_err(|error| PathwayGseaError::Runtime(error.to_string()))?);
    }
    let batch = if batches.is_empty() {
        RecordBatch::new_empty(report_schema())
    } else {
        arrow::compute::concat_batches(&report_schema(), &batches)
            .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?
    };
    ctx.session()
        .read_batch(batch)
        .map_err(|error| PathwayGseaError::Runtime(error.to_string()))
}

async fn cleanup_staging(ctx: &NodeCtx, paths: &[String], reporter: &NodeReporter) {
    if let Ok(storage) = require_storage(ctx) {
        for path in paths {
            if let Err(error) = storage
                .resolve(path)
                .delete(&storage.resolve_path(path))
                .await
            {
                reporter.warn(format!(
                    "cannot remove pathway-gsea staging input `{path}`: {error}"
                ));
            }
        }
    }
}

#[async_trait]
impl DagNode for PathwayGseaContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        PATHWAY_GSEA_CONTAINER_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if inputs.iter().any(|input| input.port > 1) {
            return Err(PathwayGseaError::Invalid(
                "valid input ports are 0 (rank table) and 1 (gene sets)".into(),
            )
            .into());
        }
        if (input_dataframe(inputs, 0).is_none() && input_file(inputs, 0).is_none())
            || (input_dataframe(inputs, 1).is_none() && input_file(inputs, 1).is_none())
        {
            return Err(PathwayGseaError::Invalid(
                "input 0 requires a rank DataFrame or .tsv File; input 1 requires a pathway DataFrame or .gmt File".into(),
            )
                .into());
        }

        let mut container_inputs = Vec::with_capacity(inputs.len());
        let mut staging_paths = Vec::new();
        let prefix = staging_prefix();
        for port in [0_u8, 1_u8] {
            if let Some(df) = input_dataframe(inputs, port) {
                let path = if port == 0 {
                    format!("{prefix}/rank.tsv")
                } else {
                    format!("{prefix}/pathways.tsv")
                };
                stage_dataframe(ctx, df, &path).await?;
                staging_paths.push(path.clone());
                container_inputs.push(NodeInput {
                    port,
                    data: FileRef::new(path, Some("tsv".into())).into(),
                });
            } else if let Some(file) = input_file(inputs, port) {
                let path = file_extension(&file.path);
                let expected = if port == 0 { ".tsv" } else { ".gmt" };
                if !path.ends_with(expected) {
                    return Err(PathwayGseaError::Invalid(format!(
                        "input {port} file must use the {expected} extension"
                    ))
                    .into());
                }
                container_inputs.push(NodeInput {
                    port,
                    data: file.clone().into(),
                });
            }
        }

        let run_result = self.inner.execute(ctx, &container_inputs, reporter).await;
        if !staging_paths.is_empty() {
            cleanup_staging(ctx, &staging_paths, reporter).await;
        }
        let outputs = run_result?;

        let report_tsv = outputs
            .get(&0)
            .ok_or_else(|| {
                PathwayGseaError::Runtime("container did not return a TSV report".into())
            })?
            .as_file()
            .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?
            .clone();
        let artifact = outputs
            .get(&1)
            .ok_or_else(|| {
                PathwayGseaError::Runtime("container did not return a JSON artifact".into())
            })?
            .as_file()
            .map_err(|error| PathwayGseaError::Runtime(error.to_string()))?
            .clone();
        let dataframe = report_to_dataframe(ctx, &report_tsv).await?;

        let mut result = PortOutputs::new();
        result.insert(0, dataframe);
        result.insert_file(1, artifact);
        Ok(result)
    }
}

impl NodeFactory for PathwayGseaContainerNodeFactory {
    fn kind(&self) -> &'static str {
        PATHWAY_GSEA_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs preranked GSEA with Bioconductor fgsea in a pinned container."
    }

    fn doc(&self) -> &'static str {
        "Input port 0 is a gene/score DataFrame or .tsv File; input port 1 is \
        a long-format pathway DataFrame or .gmt File. Calls fgseaMultilevel \
        and emits pathway, pval, padj, NES, size, leadingEdge plus a JSON \
        artifact. The container is isolated and its root filesystem is read-only."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PathwayGseaContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PathwayGseaContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let inner = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(PathwayGseaContainerNode {
            ports: port_layout(),
            spec,
            inner: Box::new(inner),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: PathwayGseaContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float64Array, StringArray};
    use container_runtime::{ContainerRunRequest, ContainerRunResult, ContainerRuntimeError};
    use std::path::{Path, PathBuf};
    use vfs::OpendalFileStorage;

    fn spec() -> PathwayGseaContainerSpec {
        PathwayGseaContainerSpec {
            gene_col: "gene".into(),
            score_col: "score".into(),
            pathway_col: "pathway_name".into(),
            pathway_gene_col: "gene".into(),
            min_size: 15,
            max_size: 500,
            eps: 1e-50,
            artifact_prefix: DEFAULT_ARTIFACT_PREFIX.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cpus: None,
            memory: None,
        }
    }

    #[test]
    fn validates_parameters_and_defaults() {
        let valid = spec();
        assert!(validate(&valid).is_ok());
        assert_eq!(valid.gene_col, "gene");
        assert_eq!(valid.score_col, "score");
        assert_eq!(valid.min_size, 15);
        assert_eq!(valid.max_size, 500);
        assert_eq!(valid.eps, 1e-50);

        let mut empty_column = spec();
        empty_column.pathway_col = " ".into();
        assert!(validate(&empty_column).is_err());

        let mut range = spec();
        range.max_size = 1;
        assert!(validate(&range).is_err());

        let mut eps = spec();
        eps.eps = 0.0;
        assert!(validate(&eps).is_err());
    }

    #[test]
    fn declares_ports_and_pinned_container_contract() {
        let ports = port_layout();
        assert_eq!(ports.input_ports().len(), 2);
        assert_eq!(ports.output_ports().len(), 2);

        let container = container_spec(&spec()).unwrap();
        assert!(
            container
                .image
                .ends_with(&format!("@{PATHWAY_GSEA_IMAGE_DIGEST}"))
        );
        assert_eq!(container.command, vec!["Rscript"]);
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.cpus, Some(DEFAULT_CPUS));
        assert_eq!(container.memory.as_deref(), Some(DEFAULT_MEMORY));
        assert_eq!(container.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(container.script.unwrap().contains("fgsea::fgseaMultilevel"));
    }

    struct FakeRuntime {
        workspace_root: Option<PathBuf>,
        requests: std::sync::Mutex<Vec<ContainerRunRequest>>,
    }

    impl FakeRuntime {
        fn new(workspace_root: &Path) -> Self {
            Self {
                workspace_root: Some(workspace_root.to_path_buf()),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn value(env: &[(String, String)], name: &str) -> String {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| panic!("missing {name}"))
        }

        fn host_path(&self, request: &ContainerRunRequest, path: &str) -> PathBuf {
            let relative = path
                .strip_prefix(container_runtime::DEFAULT_CONTAINER_WORKDIR)
                .unwrap()
                .trim_start_matches('/');
            request.workspace.host_path.join(relative)
        }
    }

    #[async_trait::async_trait]
    impl PodmanConnection for FakeRuntime {
        async fn run(
            &self,
            request: ContainerRunRequest,
        ) -> Result<ContainerRunResult, ContainerRuntimeError> {
            let env = request.env.clone();
            let report = self.host_path(&request, &Self::value(&env, "AUTONOMICS_OUTPUT0"));
            let json = self.host_path(&request, &Self::value(&env, "AUTONOMICS_OUTPUT1"));
            std::fs::create_dir_all(report.parent().unwrap()).unwrap();
            std::fs::write(
                report,
                "pathway\tpval\tpadj\tNES\tsize\tleadingEdge\nPATHWAY\t0.01\t0.02\t1.5\t3\tA;C\n",
            )
            .unwrap();
            std::fs::write(json, br#"{"method":"fgseaMultilevel"}"#).unwrap();
            assert_eq!(Self::value(&env, "AUTONOMICS_GENE_COL"), "gene");
            assert_eq!(Self::value(&env, "AUTONOMICS_SCORE_COL"), "score");
            assert_eq!(Self::value(&env, "AUTONOMICS_PATHWAY_COL"), "pathway_name");
            assert!(Self::value(&env, "AUTONOMICS_INPUT0").ends_with(".tsv"));
            assert!(Self::value(&env, "AUTONOMICS_INPUT1").ends_with(".tsv"));
            self.requests.lock().unwrap().push(request);
            Ok(ContainerRunResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }

        fn name(&self) -> &'static str {
            "fake-pathway-gsea"
        }

        fn workspace_root(&self) -> &Path {
            self.workspace_root.as_deref().unwrap_or(Path::new("/tmp"))
        }
    }

    #[tokio::test]
    async fn stages_dataframes_and_wraps_container_outputs() {
        let root = tempfile::tempdir().unwrap();
        let (session, storage) = OpendalFileStorage::new_temp().register_to_ctx();
        let node_ctx = NodeCtx::new(session.runtime_env().clone(), Some(storage));

        let rank = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("gene", DataType::Utf8, false),
                Field::new("score", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["A", "B", "C"])),
                Arc::new(Float64Array::from(vec![2.0, 1.0, 0.5])),
            ],
        )
        .unwrap();
        let pathways = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("pathway_name", DataType::Utf8, false),
                Field::new("gene", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["PATHWAY"; 3])),
                Arc::new(StringArray::from(vec!["A", "B", "C"])),
            ],
        )
        .unwrap();
        let session = node_ctx.session();
        let inputs = vec![
            NodeInput::new_dataframe(0, session.read_batch(rank).unwrap()),
            NodeInput::new_dataframe(1, session.read_batch(pathways).unwrap()),
        ];

        let runtime = Arc::new(FakeRuntime::new(root.path()));
        let factory = PathwayGseaContainerNodeFactory::new(
            runtime.clone(),
            Arc::new(PanelCache::new(root.path().join("panels"))),
        );
        let mut node = factory
            .build(
                serde_json::json!({
                    "min_size": 1,
                    "max_size": 100,
                    "artifact_prefix": "/artifacts/pathway-test"
                }),
                node_ctx.clone(),
            )
            .unwrap();

        let outputs = node
            .execute(&node_ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();
        let dataframe = outputs.get(&0).unwrap().as_dataframe().unwrap();
        let batches = dataframe.clone().collect().await.unwrap();
        assert_eq!(batches[0].num_rows(), 1);
        assert_eq!(
            dataframe.schema().inner().as_ref(),
            report_schema().as_ref()
        );

        let artifact = outputs.get(&1).unwrap().as_file().unwrap();
        assert!(artifact.path.starts_with("vfs:///artifacts/pathway-test/"));
        assert!(artifact.path.ends_with("/fgsea_report.json"));
        assert!(
            artifact
                .fingerprint
                .as_ref()
                .unwrap()
                .content_hash
                .is_some()
        );

        let request = runtime.requests.lock().unwrap()[0].clone();
        let staged_rank = request
            .env
            .iter()
            .find(|(name, _)| name == "AUTONOMICS_INPUT0")
            .map(|(_, value)| value.as_str())
            .unwrap();
        let relative = staged_rank
            .strip_prefix(container_runtime::DEFAULT_CONTAINER_WORKDIR)
            .unwrap();
        assert!(
            node_ctx
                .opendal
                .as_ref()
                .unwrap()
                .resolve(relative)
                .stat(&node_ctx.opendal.as_ref().unwrap().resolve_path(relative))
                .await
                .is_err()
        );
    }
}
