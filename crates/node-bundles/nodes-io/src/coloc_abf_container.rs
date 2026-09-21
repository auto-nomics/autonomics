//! Containerized COLOC `coloc.abf` node backed by the official R package.
//!
//! Thin wrapper that executes `coloc::coloc.abf()` from the pinned official R
//! `coloc` package (CRAN 5.2.3, Wallace). It accepts a single tab-separated
//! File with two GWAS summary-statistic datasets merged on SNP, writes the
//! official coloc result object as an RDS artifact, and emits a printed log.
//!
//! No reference panel, no LD matrix, no GWAS inputs are baked into the
//! image; the wrapper delegates execution to [`ContainerCommandNode`] and
//! does not reimplement any coloc numerical logic.

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

pub const COLOC_ABF_CONTAINER_KIND: &str = "coloc_abf_container";
pub const COLOC_ORIGINAL_IMAGE_REPOSITORY: &str = "coloc";
pub const COLOC_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:a2afe7aa83ae6f1ecb9573317aa022af4db0870771378c7fe90d295d70057322";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/coloc_abf_container";
const DEFAULT_TIMEOUT_SECS: u64 = 600;

const DEFAULT_PRIOR_P1: f64 = 1e-4;
const DEFAULT_PRIOR_P2: f64 = 1e-4;
const DEFAULT_PRIOR_P12: f64 = 1e-5;

/// Trait type passed to `coloc::coloc.abf()`. Mirrors the R package vocabulary.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ColocTraitType {
    /// Quantitative trait. Renders as `"quant"` in the R script.
    Quant,
    /// Case-control trait. Renders as `"cc"` in the R script.
    Cc,
}

fn trait_type_str(value: ColocTraitType) -> &'static str {
    match value {
        ColocTraitType::Quant => "quant",
        ColocTraitType::Cc => "cc",
    }
}

/// Per-trait summary-statistic column mapping.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ColocDatasetSpec {
    /// `"quant"` or `"cc"`.
    pub r#type: ColocTraitType,
    /// Column name for SNP identifiers.
    pub snp: String,
    /// Column name for regression coefficients (preferred path).
    #[serde(default)]
    pub beta: Option<String>,
    /// Column name for the variance of `beta`.
    #[serde(default)]
    pub varbeta: Option<String>,
    /// Column name for p-values (alternative to `beta`/`varbeta`).
    #[serde(default)]
    pub pvalues: Option<String>,
    /// Column name for minor allele frequency (required with `pvalues`).
    #[serde(default)]
    pub maf: Option<String>,
    /// Sample size (scalar). Required when using `pvalues`.
    #[serde(default)]
    pub n: Option<f64>,
    /// For case-control: proportion of cases (required with `pvalues`).
    #[serde(default)]
    pub s: Option<f64>,
    /// For quantitative: population SD of the trait (optional).
    #[serde(default)]
    pub sd_y: Option<f64>,
}

impl ColocDatasetSpec {
    fn validate(&self, role: &str) -> Result<(), String> {
        if self.snp.trim().is_empty() {
            return Err(format!("{role}.snp cannot be empty"));
        }
        match (
            self.beta.as_ref(),
            self.varbeta.as_ref(),
            self.pvalues.as_ref(),
        ) {
            (Some(b), Some(v), None) => {
                if b.trim().is_empty() || v.trim().is_empty() {
                    return Err(format!("{role}.beta and {role}.varbeta cannot be empty"));
                }
            }
            (None, None, Some(p)) => {
                if p.trim().is_empty() {
                    return Err(format!("{role}.pvalues cannot be empty"));
                }
                if self.maf.is_none() {
                    return Err(format!("{role}.maf is required when using {role}.pvalues"));
                }
                if self.n.is_none() {
                    return Err(format!("{role}.n is required when using {role}.pvalues"));
                }
                if matches!(self.r#type, ColocTraitType::Cc) && self.s.is_none() {
                    return Err(format!("{role}.s is required for cc + pvalues"));
                }
            }
            _ => {
                return Err(format!(
                    "{role} must supply exactly one of (beta+varbeta) or pvalues"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ColocAbfContainerSpec {
    pub dataset1: ColocDatasetSpec,
    pub dataset2: ColocDatasetSpec,
    /// Prior probability that a SNP is associated with trait 1.
    #[serde(default = "default_prior_p1")]
    pub p1: f64,
    /// Prior probability that a SNP is associated with trait 2.
    #[serde(default = "default_prior_p2")]
    pub p2: f64,
    /// Prior probability that a SNP is associated with both traits.
    #[serde(default = "default_prior_p12")]
    pub p12: f64,
    /// VFS prefix used to publish declared outputs.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Wall-clock timeout for the container, in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_prior_p1() -> f64 {
    DEFAULT_PRIOR_P1
}

fn default_prior_p2() -> f64 {
    DEFAULT_PRIOR_P2
}

fn default_prior_p12() -> f64 {
    DEFAULT_PRIOR_P12
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct ColocAbfContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl ColocAbfContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct ColocAbfContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for ColocAbfContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for ColocAbfContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        COLOC_ABF_CONTAINER_KIND
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

fn r_string(value: &str) -> String {
    format!("{value:?}")
}

pub fn validate(spec: &ColocAbfContainerSpec) -> Result<(), String> {
    spec.dataset1.validate("dataset1")?;
    spec.dataset2.validate("dataset2")?;
    if !(0.0..=1.0).contains(&spec.p1) {
        return Err("p1 must be in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.p2) {
        return Err("p2 must be in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.p12) {
        return Err("p12 must be in (0, 1]".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn dataset_block(spec: &ColocDatasetSpec, idx: u8) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "snp_{idx} <- data${col}",
        col = r_string(&spec.snp)
    ));
    lines.push(format!(
        "type_{idx} <- {ty}",
        ty = r_string(trait_type_str(spec.r#type))
    ));
    if let Some(beta) = &spec.beta {
        lines.push(format!("beta_{idx} <- data${col}", col = r_string(beta)));
    }
    if let Some(varbeta) = &spec.varbeta {
        lines.push(format!(
            "varbeta_{idx} <- data${col}",
            col = r_string(varbeta)
        ));
    }
    if let Some(pvalues) = &spec.pvalues {
        lines.push(format!(
            "pvalues_{idx} <- data${col}",
            col = r_string(pvalues)
        ));
    }
    if let Some(maf) = &spec.maf {
        lines.push(format!("MAF_{idx} <- data${col}", col = r_string(maf)));
    }
    if let Some(n) = spec.n {
        lines.push(format!("N_{idx} <- {n}"));
    }
    if let Some(s) = spec.s {
        lines.push(format!("s_{idx} <- {s}"));
    }
    if let Some(sd_y) = spec.sd_y {
        lines.push(format!("sdY_{idx} <- {sd_y}"));
    }
    let mut list_args: Vec<String> = vec![format!("snp = snp_{idx}"), format!("type = type_{idx}")];
    if spec.beta.is_some() {
        list_args.push(format!("beta = beta_{idx}"));
    }
    if spec.varbeta.is_some() {
        list_args.push(format!("varbeta = varbeta_{idx}"));
    }
    if spec.pvalues.is_some() {
        list_args.push(format!("pvalues = pvalues_{idx}"));
    }
    if spec.maf.is_some() {
        list_args.push(format!("MAF = MAF_{idx}"));
    }
    if spec.n.is_some() {
        list_args.push(format!("N = N_{idx}"));
    }
    if spec.s.is_some() {
        list_args.push(format!("s = s_{idx}"));
    }
    if spec.sd_y.is_some() {
        list_args.push(format!("sdY = sdY_{idx}"));
    }
    lines.push(format!(
        "dataset_{idx} <- list({args})",
        args = list_args.join(", ")
    ));
    lines.join("\n")
}

// R script template. `r###"..."###` lets the script body contain `##` and `###`
// without breaking the raw-string lexer under Rust 2024's multi-hash rule.
const R_SCRIPT_TEMPLATE: &str = r###"
input <- Sys.getenv("AUTONOMICS_INPUT0")
result_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
log_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)

__DATASET_1__

__DATASET_2__

result <- coloc::coloc.abf(
  dataset1 = dataset_1,
  dataset2 = dataset_2,
  p1 = __P1__,
  p2 = __P2__,
  p12 = __P12__
)

sink(log_path, split = TRUE)
cat("## coloc.abf (official R package 5.2.3)\n")
print(result$summary)
cat("\n## top SNPs by SNP.PP.H4\n")
ord <- order(result$results$SNP.PP.H4, decreasing = TRUE)
top <- head(result$results[ord, c("snp", "SNP.PP.H4")], 10)
print(top)
sink()

saveRDS(result, result_path)
"###;

pub fn container_spec(spec: &ColocAbfContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let r_code = R_SCRIPT_TEMPLATE
        .replace("__DATASET_1__", &dataset_block(&spec.dataset1, 1))
        .replace("__DATASET_2__", &dataset_block(&spec.dataset2, 2))
        .replace("__P1__", &format!("{}", spec.p1))
        .replace("__P2__", &format!("{}", spec.p2))
        .replace("__P12__", &format!("{}", spec.p12));

    Ok(ContainerCommandSpec {
        image: registry_image(COLOC_ORIGINAL_IMAGE_REPOSITORY, COLOC_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["Rscript".into()],
        script: Some(r_code),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "coloc_abf.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "coloc_abf.log".into(),
                format: Some("coloc_abf_log".into()),
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

const DESC: &str = "Runs official coloc.abf in an ephemeral OCI container.";

const DOC: &str = "Runs the official R `coloc` package (Wallace, CRAN 5.2.3). \
The input is one tab-separated File containing two GWAS summary-statistic \
datasets merged on SNP. The wrapper calls `coloc::coloc.abf()` directly, \
emits the official result list as an RDS artifact, and publishes a printed \
log. It executes the official package verbatim and does not reimplement any \
coloc numerical logic.";

impl NodeFactory for ColocAbfContainerNodeFactory {
    fn kind(&self) -> &'static str {
        COLOC_ABF_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        DESC
    }

    fn doc(&self) -> &'static str {
        DOC
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ColocAbfContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ColocAbfContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(ColocAbfContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: ColocAbfContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ColocAbfContainerSpec {
        ColocAbfContainerSpec {
            dataset1: ColocDatasetSpec {
                r#type: ColocTraitType::Quant,
                snp: "snp".into(),
                beta: Some("beta1".into()),
                varbeta: Some("varbeta1".into()),
                pvalues: None,
                maf: Some("maf".into()),
                n: Some(1000.0),
                s: None,
                sd_y: Some(1.0),
            },
            dataset2: ColocDatasetSpec {
                r#type: ColocTraitType::Quant,
                snp: "snp".into(),
                beta: Some("beta2".into()),
                varbeta: Some("varbeta2".into()),
                pvalues: None,
                maf: Some("maf".into()),
                n: Some(1000.0),
                s: None,
                sd_y: Some(1.0),
            },
            p1: default_prior_p1(),
            p2: default_prior_p2(),
            p12: default_prior_p12(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_coloc_abf_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            registry_image(COLOC_ORIGINAL_IMAGE_REPOSITORY, COLOC_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert!(container.panel_bundles.is_empty());
        assert!(container.panels.is_empty());
        assert_eq!(container.command, vec!["Rscript".to_string()]);
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("coloc::coloc.abf"));
        assert!(script.contains("dataset1 = dataset_1"));
        assert!(script.contains("dataset2 = dataset_2"));
        assert!(script.contains("saveRDS"));
        assert_eq!(container.outputs.len(), 2);
    }

    #[test]
    fn rejects_missing_paths() {
        let mut value = spec();
        value.dataset1.beta = None;
        value.dataset1.varbeta = None;
        value.dataset1.pvalues = None;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_pvalue_path_without_maf() {
        let mut value = spec();
        value.dataset1.beta = None;
        value.dataset1.varbeta = None;
        value.dataset1.pvalues = Some("p1".into());
        value.dataset1.maf = None;
        value.dataset1.n = Some(1000.0);
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_prior_out_of_range() {
        let mut value = spec();
        value.p12 = 1.5;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_relative_artifact_prefix() {
        let mut value = spec();
        value.artifact_prefix = "artifacts/coloc_abf".into();
        assert!(validate(&value).is_err());
    }
}
