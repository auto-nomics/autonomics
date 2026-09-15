//! Containerized maftools mutation-analysis node.
//!
//! The wrapper owns the MAF/clinical file contract and operation parameters.
//! All biological computation remains in the pinned R maftools image.

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

pub const MUTATION_ANALYSIS_CONTAINER_KIND: &str = "mutation_analysis_container";
pub const MUTATION_ANALYSIS_IMAGE_REPOSITORY: &str = "mutation-analysis";
pub const MUTATION_ANALYSIS_IMAGE_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/mutation_analysis_container";
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "8Gi";
const DEFAULT_PIDS_LIMIT: i64 = 512;
const DEFAULT_PANEL_SIZE_MB: f64 = 38.0;
const DEFAULT_TOP_N: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MutationAnalysisOperation {
    Tmb,
    Summary,
    TopGenes,
    Titv,
    Mutex,
}

impl MutationAnalysisOperation {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Tmb => "tmb",
            Self::Summary => "summary",
            Self::TopGenes => "top_genes",
            Self::Titv => "titv",
            Self::Mutex => "mutex",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MutationAnalysisContainerSpec {
    /// Provenance label for the MAF File connected to input port 0.
    pub maf_path: String,
    /// Optional provenance label; its presence enables the clinical input port.
    pub clinical_path: Option<String>,
    #[serde(default = "default_operation")]
    pub operation: MutationAnalysisOperation,
    #[serde(default = "default_panel_size_mb")]
    pub panel_size_mb: f64,
    #[serde(default = "default_tmb_group_col")]
    pub tmb_group_col: String,
    #[serde(default)]
    pub tmb_groups: Vec<String>,
    #[serde(default = "default_top_n")]
    pub top_n: usize,
    #[serde(default = "default_gene_col")]
    pub gene_col: String,
    #[serde(default = "default_variant_col")]
    pub variant_col: String,
    #[serde(default = "default_tumor_sample_col")]
    pub tumor_sample_col: String,
    #[serde(default = "default_variant_type_col")]
    pub variant_type_col: String,
    #[serde(default = "default_chromosome_col")]
    pub chromosome_col: String,
    #[serde(default = "default_start_position_col")]
    pub start_position_col: String,
    #[serde(default = "default_end_position_col")]
    pub end_position_col: String,
    #[serde(default = "default_reference_allele_col")]
    pub reference_allele_col: String,
    #[serde(default = "default_tumor_seq_allele_col")]
    pub tumor_seq_allele_col: String,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

fn default_operation() -> MutationAnalysisOperation {
    MutationAnalysisOperation::Tmb
}

fn default_panel_size_mb() -> f64 {
    DEFAULT_PANEL_SIZE_MB
}

fn default_tmb_group_col() -> String {
    "group".into()
}

fn default_top_n() -> usize {
    DEFAULT_TOP_N
}

fn default_gene_col() -> String {
    "Hugo_Symbol".into()
}

fn default_variant_col() -> String {
    "Variant_Classification".into()
}

fn default_tumor_sample_col() -> String {
    "Tumor_Sample_Barcode".into()
}

fn default_variant_type_col() -> String {
    "Variant_Type".into()
}

fn default_chromosome_col() -> String {
    "Chromosome".into()
}

fn default_start_position_col() -> String {
    "Start_Position".into()
}

fn default_end_position_col() -> String {
    "End_Position".into()
}

fn default_reference_allele_col() -> String {
    "Reference_Allele".into()
}

fn default_tumor_seq_allele_col() -> String {
    "Tumor_Seq_Allele2".into()
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct MutationAnalysisContainerNodeFactory {
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl MutationAnalysisContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct MutationAnalysisContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for MutationAnalysisContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MutationAnalysisContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MUTATION_ANALYSIS_CONTAINER_KIND
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

fn nonempty(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{name} cannot be empty"))
    } else {
        Ok(())
    }
}

pub fn validate(spec: &MutationAnalysisContainerSpec) -> Result<(), String> {
    nonempty(&spec.maf_path, "maf_path")?;
    if let Some(path) = &spec.clinical_path {
        nonempty(path, "clinical_path")?;
    }
    if !spec.panel_size_mb.is_finite() || spec.panel_size_mb <= 0.0 {
        return Err("panel_size_mb must be finite and greater than zero".into());
    }
    if spec.top_n == 0 {
        return Err("top_n must be greater than zero".into());
    }
    for (value, name) in [
        (&spec.tmb_group_col, "tmb_group_col"),
        (&spec.gene_col, "gene_col"),
        (&spec.variant_col, "variant_col"),
        (&spec.tumor_sample_col, "tumor_sample_col"),
        (&spec.variant_type_col, "variant_type_col"),
        (&spec.chromosome_col, "chromosome_col"),
        (&spec.start_position_col, "start_position_col"),
        (&spec.end_position_col, "end_position_col"),
        (&spec.reference_allele_col, "reference_allele_col"),
        (&spec.tumor_seq_allele_col, "tumor_seq_allele_col"),
    ] {
        nonempty(value, name)?;
    }
    let mapped_columns = [
        &spec.gene_col,
        &spec.variant_col,
        &spec.tumor_sample_col,
        &spec.variant_type_col,
        &spec.chromosome_col,
        &spec.start_position_col,
        &spec.end_position_col,
        &spec.reference_allele_col,
        &spec.tumor_seq_allele_col,
    ];
    let mut unique_columns = mapped_columns.iter().collect::<Vec<_>>();
    unique_columns.sort_unstable();
    unique_columns.dedup();
    if unique_columns.len() != mapped_columns.len() {
        return Err("MAF column mappings must be unique".into());
    }
    if !spec.tmb_groups.is_empty() {
        if spec.operation != MutationAnalysisOperation::Tmb {
            return Err("tmb_groups are only valid for operation `tmb`".into());
        }
        if spec.tmb_groups.len() != 2
            || spec.tmb_groups.iter().any(|group| group.trim().is_empty())
            || spec.tmb_groups[0] == spec.tmb_groups[1]
        {
            return Err("tmb_groups must contain two distinct nonempty labels".into());
        }
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
    Ok(())
}

fn r_json_array(values: &[String]) -> serde_json::Value {
    serde_json::Value::Array(
        values
            .iter()
            .map(|value| serde_json::Value::String(value.clone()))
            .collect(),
    )
}

pub fn container_spec(
    spec: &MutationAnalysisContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let config = serde_json::json!({
        "operation": spec.operation.as_label(),
        "panel_size_mb": spec.panel_size_mb,
        "tmb_group_col": spec.tmb_group_col,
        "tmb_groups": r_json_array(&spec.tmb_groups),
        "top_n": spec.top_n,
        "gene_col": spec.gene_col,
        "variant_col": spec.variant_col,
        "tumor_sample_col": spec.tumor_sample_col,
        "variant_type_col": spec.variant_type_col,
        "chromosome_col": spec.chromosome_col,
        "start_position_col": spec.start_position_col,
        "end_position_col": spec.end_position_col,
        "reference_allele_col": spec.reference_allele_col,
        "tumor_seq_allele_col": spec.tumor_seq_allele_col,
    });
    let env = BTreeMap::from([("MUTATION_CONFIG".into(), config.to_string())]);

    Ok(ContainerCommandSpec {
        image: acr_image(
            MUTATION_ANALYSIS_IMAGE_REPOSITORY,
            MUTATION_ANALYSIS_IMAGE_DIGEST,
        )?,
        command: vec![
            "Rscript".into(),
            "/opt/autonomics/mutation_analysis.R".into(),
        ],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "mutation_analysis_report.tsv".into(),
                format: Some("mutation_analysis_report_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "mutation_analysis_details.json".into(),
                format: Some("mutation_analysis_details_json".into()),
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
        pids_limit: Some(spec.pids_limit.unwrap_or(DEFAULT_PIDS_LIMIT)),
        shm_size: Some("1Gi".into()),
        user: None,
    })
}

fn port_layout(with_clinical: bool) -> NodePorts {
    let ports = NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "maf")
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File);
    if with_clinical {
        ports.add_optional_input_port_of_type(PortType::File)
    } else {
        ports
    }
}

impl NodeFactory for MutationAnalysisContainerNodeFactory {
    fn kind(&self) -> &'static str {
        MUTATION_ANALYSIS_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs pinned R maftools mutation analyses in Podman."
    }

    fn doc(&self) -> &'static str {
        "Input port 0 is a MAF File; the optional clinical File enables grouped \
        TMB comparisons. The operation emits a TSV report and a SHA-256 \
        fingerprinted JSON artifact through VFS. Supported operations are tmb, \
        summary, top_genes, titv, and mutex. The network is disabled and the \
        root filesystem is read-only."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MutationAnalysisContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout(false)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: MutationAnalysisContainerSpec = serde_json::from_value(spec)?;
        let with_clinical = spec.clinical_path.is_some();
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let node = ContainerCommandNode::new(
            container_spec,
            Arc::clone(&self.runtime),
            Arc::clone(&self.panel_cache),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(MutationAnalysisContainerNode {
            ports: port_layout(with_clinical),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: MutationAnalysisContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout(spec.clinical_path.is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MutationAnalysisContainerSpec {
        MutationAnalysisContainerSpec {
            maf_path: "/data/sample.maf".into(),
            clinical_path: Some("/data/clinical.tsv".into()),
            operation: MutationAnalysisOperation::Tmb,
            panel_size_mb: DEFAULT_PANEL_SIZE_MB,
            tmb_group_col: "group".into(),
            tmb_groups: vec!["primary".into(), "control".into()],
            top_n: DEFAULT_TOP_N,
            gene_col: "Hugo_Symbol".into(),
            variant_col: "Variant_Classification".into(),
            tumor_sample_col: "Tumor_Sample_Barcode".into(),
            variant_type_col: "Variant_Type".into(),
            chromosome_col: "Chromosome".into(),
            start_position_col: "Start_Position".into(),
            end_position_col: "End_Position".into(),
            reference_allele_col: "Reference_Allele".into(),
            tumor_seq_allele_col: "Tumor_Seq_Allele2".into(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            cpus: None,
            memory: None,
            pids_limit: None,
        }
    }

    #[test]
    fn digest_constant_is_a_sha256_manifest() {
        let digest = MUTATION_ANALYSIS_IMAGE_DIGEST;
        assert!(digest.starts_with("sha256:"));
        let hex = digest.strip_prefix("sha256:").unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn builds_isolated_contract_and_ports() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(container.command[0], "Rscript");
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.memory.as_deref(), Some(DEFAULT_MEMORY));
        let config: serde_json::Value =
            serde_json::from_str(&container.env["MUTATION_CONFIG"]).unwrap();
        assert_eq!(config["operation"], "tmb");
        assert_eq!(
            config["tmb_groups"],
            serde_json::json!(["primary", "control"])
        );
        assert_eq!(port_layout(true).input_ports().len(), 2);
        assert_eq!(port_layout(false).input_ports().len(), 1);
        assert_eq!(port_layout(true).output_ports().len(), 2);
    }

    #[test]
    fn rejects_invalid_tmb_and_resource_settings() {
        let mut invalid = spec();
        invalid.panel_size_mb = 0.0;
        assert!(validate(&invalid).unwrap_err().contains("panel_size_mb"));

        invalid = spec();
        invalid.tmb_groups = vec!["primary".into(), "primary".into()];
        assert!(validate(&invalid).unwrap_err().contains("two distinct"));

        invalid = spec();
        invalid.operation = MutationAnalysisOperation::Summary;
        invalid.tmb_groups = vec!["primary".into(), "control".into()];
        assert!(validate(&invalid).unwrap_err().contains("only valid"));

        invalid = spec();
        invalid.cpus = Some(0.0);
        assert!(validate(&invalid).unwrap_err().contains("cpus"));
    }

    #[test]
    fn clinical_path_controls_the_optional_input_port() {
        let mut without_clinical = spec();
        without_clinical.clinical_path = None;
        let ports = port_layout(false);
        assert_eq!(ports.input_ports().len(), 1);
        assert_eq!(
            container_spec(&without_clinical).unwrap().env["MUTATION_CONFIG"],
            container_spec(&spec()).unwrap().env["MUTATION_CONFIG"]
        );
    }
}
