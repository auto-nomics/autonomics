//! Containerized HyPrColoc node backed by the official R package.
//!
//! The wrapper constructs the official `effect.est` and `effect.se` matrices
//! from one SNP-aligned summary-statistic table, then delegates all numerical
//! analysis to `hyprcoloc::hyprcoloc()`. No GWAS data or correlation matrices
//! are embedded in the image.

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

pub const HYPRCOLOC_CONTAINER_KIND: &str = "hyprcoloc_container";
pub const HYPRCOLOC_ORIGINAL_IMAGE_REPOSITORY: &str = "hyprcoloc";
pub const HYPRCOLOC_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:395a36903d3c3ed6c753aef85f5b0a57044f86968e4d79804e4cc85f36aa5617";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/hyprcoloc_container";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const DEFAULT_PRIOR_1: f64 = 1e-4;
const DEFAULT_PRIOR_C: f64 = 0.02;

/// Per-trait column mapping in the SNP-aligned input table.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HyPrColocTraitSpec {
    /// Trait label passed to the official `trait.names` argument.
    pub name: String,
    /// Column containing regression coefficients for this trait.
    pub beta: String,
    /// Column containing standard errors for this trait.
    pub se: String,
    /// Whether the trait is binary (`1`) rather than continuous (`0`).
    #[serde(default)]
    pub binary_outcome: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HyPrColocBbSelection {
    Regional,
    Alignment,
}

impl HyPrColocBbSelection {
    fn as_r_argument(&self) -> &'static str {
        match self {
            Self::Regional => "regional",
            Self::Alignment => "alignment",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HyPrColocContainerSpec {
    /// Column containing SNP identifiers shared by all traits.
    pub snp_column: String,
    /// Two or more trait column mappings, in the official matrix column order.
    pub traits: Vec<HyPrColocTraitSpec>,
    /// Optional subset of trait names passed to `trait.subset`.
    #[serde(default)]
    pub trait_subset: Option<Vec<String>>,
    /// Prior probability that a SNP is associated with one trait.
    #[serde(default = "default_prior_1")]
    pub prior_1: f64,
    /// Conditional prior probability of association with an additional trait.
    #[serde(default = "default_prior_c")]
    pub prior_c: f64,
    /// Optional COLOC-style prior for association with any two traits.
    #[serde(default)]
    pub prior_12: Option<f64>,
    /// Optional regional association threshold.
    #[serde(default)]
    pub reg_thresh: Option<f64>,
    /// Optional causal-alignment threshold.
    #[serde(default)]
    pub align_thresh: Option<f64>,
    /// Use conditionally uniform rather than variant-specific priors.
    #[serde(default)]
    pub uniform_priors: bool,
    /// Use the official branch-and-bound algorithm.
    #[serde(default = "default_true")]
    pub bb_alg: bool,
    /// Branch-and-bound selection mode.
    #[serde(default = "default_bb_selection")]
    pub bb_selection: HyPrColocBbSelection,
    /// Regional step parameter.
    #[serde(default = "default_reg_steps")]
    pub reg_steps: u32,
    /// Retain official posterior SNP scores in the RDS artifact.
    #[serde(default)]
    pub snpscores: bool,
    /// VFS prefix used to publish declared outputs.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Wall-clock timeout for the container, in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_prior_1() -> f64 {
    DEFAULT_PRIOR_1
}

fn default_prior_c() -> f64 {
    DEFAULT_PRIOR_C
}

fn default_true() -> bool {
    true
}

fn default_bb_selection() -> HyPrColocBbSelection {
    HyPrColocBbSelection::Regional
}

fn default_reg_steps() -> u32 {
    1
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct HyPrColocContainerNodeFactory {
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

impl HyPrColocContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct HyPrColocContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for HyPrColocContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for HyPrColocContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        HYPRCOLOC_CONTAINER_KIND
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

fn r_vector(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| r_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("c({values})")
}

fn r_bool(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

pub fn validate(spec: &HyPrColocContainerSpec) -> Result<(), String> {
    if spec.snp_column.trim().is_empty() {
        return Err("snp_column cannot be empty".into());
    }
    if spec.traits.len() < 2 {
        return Err("hyprcoloc requires at least two traits".into());
    }

    let mut names = Vec::with_capacity(spec.traits.len());
    let mut columns = Vec::with_capacity(spec.traits.len() * 2 + 1);
    columns.push(spec.snp_column.trim());
    for trait_spec in &spec.traits {
        if trait_spec.name.trim().is_empty() {
            return Err("trait name cannot be empty".into());
        }
        if trait_spec.beta.trim().is_empty() || trait_spec.se.trim().is_empty() {
            return Err(format!(
                "trait `{}` must supply non-empty beta and se columns",
                trait_spec.name
            ));
        }
        names.push(trait_spec.name.trim());
        columns.push(trait_spec.beta.trim());
        columns.push(trait_spec.se.trim());
    }
    if names.iter().collect::<std::collections::HashSet<_>>().len() != names.len() {
        return Err("trait names must be unique".into());
    }
    if columns
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != columns.len()
    {
        return Err("snp, beta, and se columns must be unique".into());
    }

    if let Some(subset) = &spec.trait_subset {
        if subset.is_empty() {
            return Err("trait_subset cannot be empty when provided".into());
        }
        let known = names.iter().collect::<std::collections::HashSet<_>>();
        for name in subset {
            if !known.contains(&name.trim()) {
                return Err(format!("unknown trait_subset entry: {name}"));
            }
        }
    }

    if !(0.0..=1.0).contains(&spec.prior_1) || spec.prior_1 == 0.0 {
        return Err("prior_1 must be in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.prior_c) || spec.prior_c == 0.0 {
        return Err("prior_c must be in (0, 1]".into());
    }
    if let Some(prior_12) = spec.prior_12 {
        if !(0.0..=1.0).contains(&prior_12) || prior_12 == 0.0 {
            return Err("prior_12 must be in (0, 1] when provided".into());
        }
    }
    for (name, value) in [
        ("reg_thresh", spec.reg_thresh),
        ("align_thresh", spec.align_thresh),
    ] {
        if let Some(value) = value {
            if !(0.0..=1.0).contains(&value) || value == 0.0 {
                return Err(format!("{name} must be in (0, 1] when provided"));
            }
        }
    }

    if spec.bb_alg {
        let minimum = if spec.uniform_priors { 0.6 } else { 0.5 };
        let reg = spec.reg_thresh.unwrap_or(minimum);
        let align = spec.align_thresh.unwrap_or(minimum);
        if reg < minimum || align < minimum {
            return Err(format!(
                "bb_alg thresholds must be at least {minimum} for this prior mode"
            ));
        }
    }
    if spec.reg_steps == 0 {
        return Err("reg_steps must be greater than zero".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

const R_SCRIPT_TEMPLATE: &str = r###"
input <- Sys.getenv("AUTONOMICS_INPUT0")
table_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
rds_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
log_path <- Sys.getenv("AUTONOMICS_OUTPUT2")

data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)
snp_column <- __SNP_COLUMN__
beta_columns <- __BETA_COLUMNS__
se_columns <- __SE_COLUMNS__
required_columns <- c(snp_column, beta_columns, se_columns)
missing_columns <- setdiff(required_columns, names(data))
if (length(missing_columns) > 0) {
  stop("missing required input columns: ", paste(missing_columns, collapse = ", "))
}

snp_id <- data[[snp_column]]
if (any(is.na(snp_id)) || anyDuplicated(snp_id) > 0) {
  stop("SNP IDs must be present and unique")
}
effect_est <- as.matrix(data[beta_columns])
effect_se <- as.matrix(data[se_columns])
if (!all(vapply(effect_est, is.numeric, logical(1))) ||
    !all(vapply(effect_se, is.numeric, logical(1)))) {
  stop("beta and se columns must be numeric")
}

result <- hyprcoloc::hyprcoloc(
  effect.est = effect_est,
  effect.se = effect_se,
  binary.outcomes = __BINARY_OUTCOMES__,
  trait.names = __TRAIT_NAMES__,
  snp.id = snp_id,
  bb.alg = __BB_ALG__,
  bb.selection = __BB_SELECTION__,
  reg.steps = __REG_STEPS__,
  prior.1 = __PRIOR_1__,
  prior.c = __PRIOR_C__,
  uniform.priors = __UNIFORM_PRIORS__,
  snpscores = __SNPSCORES____OPTIONAL_ARGS__
)

write.table(
  result$results,
  table_path,
  sep = "\t",
  quote = FALSE,
  row.names = FALSE,
  na = "NA"
)
sink(log_path, split = TRUE)
cat("## hyprcoloc (official R package 0.0.2)\n")
cat("traits:", paste(__TRAIT_NAMES__, collapse = ", "), "\n")
print(result)
sink()
saveRDS(result, rds_path)
"###;

pub fn container_spec(spec: &HyPrColocContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let names = spec
        .traits
        .iter()
        .map(|trait_spec| trait_spec.name.trim().to_owned())
        .collect::<Vec<_>>();
    let beta_columns = spec
        .traits
        .iter()
        .map(|trait_spec| trait_spec.beta.trim().to_owned())
        .collect::<Vec<_>>();
    let se_columns = spec
        .traits
        .iter()
        .map(|trait_spec| trait_spec.se.trim().to_owned())
        .collect::<Vec<_>>();
    let binary_outcomes = spec
        .traits
        .iter()
        .map(|trait_spec| u8::from(trait_spec.binary_outcome).to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let mut optional_args = String::new();
    if let Some(subset) = &spec.trait_subset {
        optional_args.push_str(&format!(",\n  trait.subset = {}", r_vector(subset)));
    }
    if let Some(value) = spec.reg_thresh {
        optional_args.push_str(&format!(",\n  reg.thresh = {value}"));
    }
    if let Some(value) = spec.align_thresh {
        optional_args.push_str(&format!(",\n  align.thresh = {value}"));
    }
    if let Some(value) = spec.prior_12 {
        optional_args.push_str(&format!(",\n  prior.12 = {value}"));
    }

    let script = R_SCRIPT_TEMPLATE
        .replace("__SNP_COLUMN__", &r_string(&spec.snp_column))
        .replace("__BETA_COLUMNS__", &r_vector(&beta_columns))
        .replace("__SE_COLUMNS__", &r_vector(&se_columns))
        .replace("__BINARY_OUTCOMES__", &format!("c({binary_outcomes})"))
        .replace("__TRAIT_NAMES__", &r_vector(&names))
        .replace("__BB_ALG__", r_bool(spec.bb_alg))
        .replace(
            "__BB_SELECTION__",
            &r_string(spec.bb_selection.as_r_argument()),
        )
        .replace("__REG_STEPS__", &spec.reg_steps.to_string())
        .replace("__PRIOR_1__", &spec.prior_1.to_string())
        .replace("__PRIOR_C__", &spec.prior_c.to_string())
        .replace("__UNIFORM_PRIORS__", r_bool(spec.uniform_priors))
        .replace("__SNPSCORES__", r_bool(spec.snpscores))
        .replace("__OPTIONAL_ARGS__", &optional_args);

    Ok(ContainerCommandSpec {
        image: acr_image(
            HYPRCOLOC_ORIGINAL_IMAGE_REPOSITORY,
            HYPRCOLOC_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["Rscript".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "hyprcoloc_results.tsv".into(),
                format: Some("hyprcoloc_results_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "hyprcoloc.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "hyprcoloc.log".into(),
                format: Some("hyprcoloc_log".into()),
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
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

const DESC: &str = "Runs official HyPrColoc in an ephemeral OCI container.";

const DOC: &str = "Runs official R `hyprcoloc` 0.0.2 (Foley and Staley). The \
input is one SNP-aligned tab-separated File with beta and standard-error \
columns for each configured trait. The wrapper calls \
`hyprcoloc::hyprcoloc()` directly, publishes the official result table and \
full R result object, and emits an execution log. It does not reimplement any \
HyPrColoc numerical logic.";

impl NodeFactory for HyPrColocContainerNodeFactory {
    fn kind(&self) -> &'static str {
        HYPRCOLOC_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        DESC
    }

    fn doc(&self) -> &'static str {
        DOC
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HyPrColocContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: HyPrColocContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(HyPrColocContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: HyPrColocContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HyPrColocContainerSpec {
        HyPrColocContainerSpec {
            snp_column: "snp".into(),
            traits: vec![
                HyPrColocTraitSpec {
                    name: "trait1".into(),
                    beta: "beta1".into(),
                    se: "se1".into(),
                    binary_outcome: false,
                },
                HyPrColocTraitSpec {
                    name: "trait2".into(),
                    beta: "beta2".into(),
                    se: "se2".into(),
                    binary_outcome: true,
                },
            ],
            trait_subset: None,
            prior_1: default_prior_1(),
            prior_c: default_prior_c(),
            prior_12: None,
            reg_thresh: None,
            align_thresh: None,
            uniform_priors: false,
            bb_alg: true,
            bb_selection: default_bb_selection(),
            reg_steps: default_reg_steps(),
            snpscores: false,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_hyprcoloc_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                HYPRCOLOC_ORIGINAL_IMAGE_REPOSITORY,
                HYPRCOLOC_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert!(container.panels.is_empty());
        assert!(container.panel_bundles.is_empty());
        assert_eq!(container.command, vec!["Rscript".to_string()]);
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("hyprcoloc::hyprcoloc"));
        assert!(script.contains("effect.est = effect_est"));
        assert!(script.contains("effect.se = effect_se"));
        assert!(script.contains("binary.outcomes = c(0, 1)"));
        assert!(script.contains("trait.names = c(\"trait1\", \"trait2\")"));
        assert!(script.contains("write.table"));
        assert!(script.contains("saveRDS"));
        assert_eq!(container.outputs.len(), 3);
    }

    #[test]
    fn rejects_fewer_than_two_traits() {
        let mut value = spec();
        value.traits.pop();
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_duplicate_columns() {
        let mut value = spec();
        value.traits[1].beta = "beta1".into();
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_unknown_trait_subset() {
        let mut value = spec();
        value.trait_subset = Some(vec!["missing".into()]);
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_bb_algorithm_below_official_threshold() {
        let mut value = spec();
        value.reg_thresh = Some(0.49);
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_relative_artifact_prefix() {
        let mut value = spec();
        value.artifact_prefix = "artifacts/hyprcoloc".into();
        assert!(validate(&value).is_err());
    }
}
