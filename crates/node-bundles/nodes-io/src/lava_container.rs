//! Containerized LAVA node backed by the official R package.
//!
//! LAVA consumes several named files and builds an opaque in-memory locus
//! object. The public node therefore takes one ZIP run bundle, invokes the
//! official `process.input -> read.loci -> process.locus -> run.*` path, and
//! owns the compatible image/reference-panel binding.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{
    DataBundleBinding, NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs,
    value::PortType,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
    ContainerPanelBundleSpec,
};
use container_runtime::{ContainerRuntime, PanelCache, PullPolicy};

pub const LAVA_CONTAINER_KIND: &str = "lava_container";
pub const LAVA_ORIGINAL_IMAGE: &str = "localhost/atc/lava:0.1.5";
pub const LAVA_TUTORIAL_REF_PANEL: &str = "lava.ref.1000g_test";
pub const LAVA_UKB_EUR_PANEL: &str = "lava.ref.ukb_eur";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/lava_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_LOCUS_INDEX: usize = 1;
const DEFAULT_MIN_K: usize = 2;
const DEFAULT_PRUNE_THRESH: f64 = 99.0;
const DEFAULT_MAX_PROP_K: f64 = 0.75;
const DEFAULT_MAX_BLOCK_SIZE: usize = 3000;
const DEFAULT_PARAM_LIM: f64 = 1.25;
const DEFAULT_MULTIREG_PARAM_LIM: f64 = 1.5;
const DEFAULT_MAX_R2: f64 = 0.95;

/// Known LD reference panel bindings (mount path + filename prefix inside the mount).
/// The `panel_id` strings match catalog package IDs.
struct PanelBinding {
    mount_path: &'static str,
    ref_prefix: &'static str,
}

const KNOWN_PANELS: &[(&str, PanelBinding)] = &[
    (
        LAVA_TUTORIAL_REF_PANEL,
        PanelBinding {
            mount_path: "/panels/lava_ref",
            ref_prefix: "g1000_test",
        },
    ),
    (
        LAVA_UKB_EUR_PANEL,
        PanelBinding {
            mount_path: "/panels/lava_ref",
            ref_prefix: "lava-ukb-v1.1",
        },
    ),
];

fn resolve_panel(panel_id: &str) -> Option<&'static PanelBinding> {
    KNOWN_PANELS
        .iter()
        .find(|(id, _)| *id == panel_id)
        .map(|(_, binding)| binding)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LavaAnalysis {
    Univ,
    Bivar,
    Pcor,
    Multireg,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LavaContainerSpec {
    /// Official LAVA analysis API to invoke.
    pub analysis: LavaAnalysis,
    /// Catalog panel ID whose LD reference the container should mount.
    /// Known panels: `lava.ref.1000g_test` (official tutorial) and
    /// `lava.ref.ukb_eur` (UK Biobank binary LD reference v1.1).
    pub panel_id: String,
    /// Optional override for the panel mount path inside the container.
    #[serde(default)]
    pub mount_path_override: Option<String>,
    /// Optional override for the filename prefix inside the mounted panel.
    #[serde(default)]
    pub ref_prefix_override: Option<String>,
    /// Input-info path relative to the ZIP bundle root.
    #[serde(default = "default_input_info_file")]
    pub input_info_file: String,
    /// Loci-table path relative to the ZIP bundle root.
    #[serde(default = "default_loci_file")]
    pub loci_file: String,
    /// Optional sample-overlap path relative to the ZIP bundle root.
    #[serde(default)]
    pub sample_overlap_file: Option<String>,
    /// One-based row in the loci table. Ignored when `locus_id` is set.
    #[serde(default = "default_locus_index")]
    pub locus_index: usize,
    /// Optional `LOC` identifier from the loci table.
    #[serde(default)]
    pub locus_id: Option<String>,
    /// Optional phenotype subset passed to `process.input`.
    #[serde(default)]
    pub phenotypes: Option<Vec<String>>,
    /// Target phenotype(s); the required shape depends on `analysis`.
    #[serde(default)]
    pub target: Option<Vec<String>>,
    /// Include local genetic variance in `run.univ` output.
    #[serde(default)]
    pub variances: bool,
    /// Adaptive p-value thresholds. `null` uses the official default.
    #[serde(default)]
    pub adap_thresh: Option<Vec<f64>>,
    /// Compute simulation p-values.
    #[serde(default = "default_true")]
    pub p_values: bool,
    /// Compute confidence intervals.
    #[serde(default = "default_true")]
    pub cis: bool,
    /// Run only the full model in `run.multireg`.
    #[serde(default)]
    pub only_full_model: bool,
    /// Maximum marginal R-squared allowed by `run.pcor`.
    #[serde(default = "default_max_r2")]
    pub max_r2: f64,
    /// VFS prefix for immutable LAVA artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_input_info_file() -> String {
    "input.info.txt".into()
}

fn default_loci_file() -> String {
    "loci.txt".into()
}

fn default_locus_index() -> usize {
    DEFAULT_LOCUS_INDEX
}

fn default_true() -> bool {
    true
}

fn default_max_r2() -> f64 {
    DEFAULT_MAX_R2
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct LavaContainerNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LavaContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LavaContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for LavaContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for LavaContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LAVA_CONTAINER_KIND
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

fn validate_bundle_path(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || std::path::Path::new(value)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("{field} must be a safe relative bundle path"));
    }
    Ok(())
}

fn validate_finite_positive(value: f64, field: &str) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{field} must be finite and greater than zero"));
    }
    Ok(())
}

pub fn validate(spec: &LavaContainerSpec) -> Result<(), String> {
    if spec.panel_id.trim().is_empty() {
        return Err("panel_id cannot be empty".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    validate_bundle_path(&spec.input_info_file, "input_info_file")?;
    validate_bundle_path(&spec.loci_file, "loci_file")?;
    if let Some(path) = &spec.sample_overlap_file {
        validate_bundle_path(path, "sample_overlap_file")?;
    }
    if spec.locus_index == 0 {
        return Err("locus_index must be one-based".into());
    }
    if let Some(id) = &spec.locus_id
        && id.trim().is_empty()
    {
        return Err("locus_id cannot be empty".into());
    }
    if let Some(phenotypes) = &spec.phenotypes
        && (phenotypes.is_empty() || phenotypes.iter().any(|value| value.trim().is_empty()))
    {
        return Err("phenotypes cannot contain empty IDs".into());
    }
    if let Some(thresholds) = &spec.adap_thresh {
        if thresholds.is_empty() {
            return Err("adap_thresh cannot be empty when provided".into());
        }
        for threshold in thresholds {
            validate_finite_positive(*threshold, "adap_thresh")?;
        }
    }
    validate_finite_positive(spec.max_r2, "max_r2")?;

    match spec.analysis {
        LavaAnalysis::Univ => {
            if spec.target.is_some() {
                return Err("target is not used by the univ analysis".into());
            }
        }
        LavaAnalysis::Bivar => {
            if let Some(target) = &spec.target
                && target.len() != 1
            {
                return Err("bivar target must contain one phenotype or be null".into());
            }
        }
        LavaAnalysis::Pcor => {
            let target = spec.target.as_ref().ok_or_else(|| {
                "pcor target must contain exactly two distinct phenotypes".to_string()
            })?;
            if target.len() != 2 || target[0] == target[1] {
                return Err("pcor target must contain exactly two distinct phenotypes".into());
            }
        }
        LavaAnalysis::Multireg => {
            let target = spec
                .target
                .as_ref()
                .ok_or_else(|| "multireg target must contain one outcome phenotype".to_string())?;
            if target.len() != 1 {
                return Err("multireg target must contain one outcome phenotype".into());
            }
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

fn optional_r_strings(values: &Option<Vec<String>>) -> String {
    values
        .as_ref()
        .map(|values| r_strings(values))
        .unwrap_or_else(|| "NULL".into())
}

fn r_bool(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

fn r_adap_thresh(values: &Option<Vec<f64>>) -> String {
    match values {
        Some(values) => {
            let values = values
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("c({values})")
        }
        None => "NULL".into(),
    }
}

pub fn container_spec(spec: &LavaContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let panel = resolve_panel(&spec.panel_id).ok_or_else(|| {
        format!(
            "unknown LAVA panel `{}`; known panels: {}, {}",
            spec.panel_id, LAVA_TUTORIAL_REF_PANEL, LAVA_UKB_EUR_PANEL
        )
    })?;
    let mount_path = spec
        .mount_path_override
        .clone()
        .unwrap_or_else(|| panel.mount_path.to_string());
    let ref_prefix = spec
        .ref_prefix_override
        .clone()
        .unwrap_or_else(|| panel.ref_prefix.to_string());
    let target = match spec.analysis {
        LavaAnalysis::Univ => "NULL".into(),
        LavaAnalysis::Bivar => spec
            .target
            .as_ref()
            .and_then(|values| values.first())
            .map(|value| r_string(value))
            .unwrap_or_else(|| "NULL".into()),
        LavaAnalysis::Pcor | LavaAnalysis::Multireg => {
            r_strings(spec.target.as_deref().unwrap_or_default())
        }
    };
    let locus_selector = spec
        .locus_id
        .as_ref()
        .map(|id| format!("match({}, as.character(loci$LOC))", r_string(id)))
        .unwrap_or_else(|| spec.locus_index.to_string());
    let param_lim = match spec.analysis {
        LavaAnalysis::Multireg => DEFAULT_MULTIREG_PARAM_LIM,
        LavaAnalysis::Univ | LavaAnalysis::Bivar | LavaAnalysis::Pcor => DEFAULT_PARAM_LIM,
    };
    let analysis_call = match spec.analysis {
        LavaAnalysis::Univ => format!(
            "result <- LAVA::run.univ(locus, phenos = phenos, var = {}, cap.estimates = TRUE)",
            r_bool(spec.variances)
        ),
        LavaAnalysis::Bivar => format!(
            "result <- LAVA::run.bivar(locus, phenos = phenos, target = {target}, adap.thresh = {}, p.values = {}, CIs = {}, param.lim = {param_lim}, cap.estimates = TRUE)",
            r_adap_thresh(&spec.adap_thresh),
            r_bool(spec.p_values),
            r_bool(spec.cis)
        ),
        LavaAnalysis::Pcor => format!(
            "result <- LAVA::run.pcor(locus, phenos = phenos, target = {target}, adap.thresh = {}, p.values = {}, CIs = {}, max.r2 = {}, param.lim = {param_lim})",
            r_adap_thresh(&spec.adap_thresh),
            r_bool(spec.p_values),
            r_bool(spec.cis),
            spec.max_r2
        ),
        LavaAnalysis::Multireg => format!(
            "result <- LAVA::run.multireg(locus, phenos = phenos, target = {target}, adap.thresh = {}, only.full.model = {}, p.values = {}, CIs = {}, param.lim = {param_lim}, suppress.message = FALSE)",
            r_adap_thresh(&spec.adap_thresh),
            r_bool(spec.only_full_model),
            r_bool(spec.p_values),
            r_bool(spec.cis)
        ),
    };
    let sample_overlap = spec
        .sample_overlap_file
        .as_deref()
        .map(r_string)
        .unwrap_or_else(|| "NULL".into());
    let analysis_name = format!("{:?}", spec.analysis).to_lowercase();
    let script = format!(
        "options(width = 200)\n\
         sink(Sys.getenv(\"AUTONOMICS_OUTPUT2\"), split = TRUE)\n\
         on.exit(sink(), add = TRUE)\n\
         bundle_dir <- file.path(Sys.getenv(\"AUTONOMICS_WORKDIR\"), \".autonomics\", \"bundle\")\n\
         dir.create(bundle_dir, recursive = TRUE, showWarnings = FALSE)\n\
         utils::unzip(Sys.getenv(\"AUTONOMICS_INPUT0\"), exdir = bundle_dir)\n\
         setwd(bundle_dir)\n\
         phenos <- {}\n\
         input <- LAVA::process.input(\n\
         \x20 input.info.file = {},\n\
         \x20 sample.overlap.file = {},\n\
         \x20 ref.prefix = \"{}/{}\",\n\
         \x20 phenos = phenos\n         )\n\
         loci <- LAVA::read.loci({})\n\
         if ({locus_selector} > nrow(loci) || is.na({locus_selector})) {{\n\
         \x20 stop(\"selected locus is outside the loci table\")\n\
         }}\n\
         locus_row <- loci[{}, , drop = FALSE]\n\
         locus <- LAVA::process.locus(\n\
         \x20 locus_row,\n\
         \x20 input,\n\
         \x20 phenos = phenos,\n\
         \x20 min.K = {},\n\
         \x20 prune.thresh = {},\n\
         \x20 max.prop.K = {},\n\
         \x20 drop.failed = TRUE,\n\
         \x20 max.block.size = {},\n\
         \x20 cap.estimates = TRUE\n\
         )\n\
         if (is.null(locus)) {{ stop(\"official LAVA could not process the selected locus\") }}\n\
         {analysis_call}\n\
         if (is.null(result)) {{ stop(\"official LAVA returned no result\") }}\n\
         flatten_result <- function(value, model = character()) {{
           if (is.data.frame(value)) {{
             output <- value
             if (length(model) > 0) output$model <- paste(model, collapse = \"/\")
             return(output)
           }}
           if (is.null(names(value)) || identical(names(value), character(0))) {{
             child_results <- lapply(value, flatten_result, model = model)
           }} else {{
             child_results <- Map(function(name, child) flatten_result(child, c(model, name)), names(value), value)
           }}
           results <- Filter(Negate(is.null), child_results)
           if (length(results) == 0) return(NULL)
           do.call(rbind, results)
         }}
         flat_result <- flatten_result(result)\n\
         print(locus_row)\n\
         print(result)\n\
         write.table(\n\
         \x20 flat_result,\n\
         \x20 Sys.getenv(\"AUTONOMICS_OUTPUT0\"),\n\
         \x20 sep = \"\\t\",\n\
         \x20 quote = FALSE,\n\
         \x20 row.names = FALSE\n\
         )\n\
         saveRDS(list(locus = locus_row, analysis = {}, result = result), Sys.getenv(\"AUTONOMICS_OUTPUT1\"))\n",
        optional_r_strings(&spec.phenotypes),
        r_string(&spec.input_info_file),
        sample_overlap,
        mount_path.as_str(),
        ref_prefix.as_str(),
        r_string(&spec.loci_file),
        locus_selector,
        DEFAULT_MIN_K,
        DEFAULT_PRUNE_THRESH,
        DEFAULT_MAX_PROP_K,
        DEFAULT_MAX_BLOCK_SIZE,
        r_string(&analysis_name),
    );

    Ok(ContainerCommandSpec {
        image: LAVA_ORIGINAL_IMAGE.into(),
        command: vec!["Rscript".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "lava.tsv".into(),
                format: Some("lava_result_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "lava.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "lava.log".into(),
                format: Some("lava_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: spec.panel_id.clone(),
            mount_path: mount_path.clone(),
        }],
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
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings_for(panel_id: &str) -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("lava_ref", panel_id)]
}

fn default_panel_bindings() -> Vec<DataBundleBinding> {
    panel_bindings_for(LAVA_UKB_EUR_PANEL)
}

impl NodeFactory for LavaContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LAVA_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs the official LAVA R package for one locus in k3s."
    }

    fn doc(&self) -> &'static str {
        "Runs the official LAVA R package in an ephemeral OCI container. Input is \
        one ZIP run bundle containing `input.info.txt`, a loci table, an optional \
        sample-overlap matrix, and all referenced sumstats files with their relative \
        paths preserved. `analysis` selects `run.univ`, `run.bivar`, `run.pcor`, or \
        `run.multireg`; exactly one locus is processed. The node owns the official \
        LAVA image and tutorial reference-panel binding, emits a flattened TSV result, \
        the raw official result as RDS, and the complete LAVA log. The tutorial panel \
        is for migration validation only and is not a production whole-genome LD panel."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        default_panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let parsed: LavaContainerSpec =
            serde_json::from_value(spec).map_err(dag_core::registry::error::Error::from)?;
        Ok(panel_bindings_for(&parsed.panel_id))
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LavaContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = container_spec
            .panel_bundles
            .iter()
            .map(|_| node_ctx.bound_data_bundle("lava_ref").cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::ContainerRuntime> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(LavaContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LavaContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(analysis: LavaAnalysis, target: Option<Vec<String>>) -> LavaContainerSpec {
        LavaContainerSpec {
            panel_id: LAVA_TUTORIAL_REF_PANEL.into(),
            mount_path_override: None,
            ref_prefix_override: None,
            analysis,
            input_info_file: default_input_info_file(),
            loci_file: default_loci_file(),
            sample_overlap_file: Some("sample.overlap.txt".into()),
            locus_index: DEFAULT_LOCUS_INDEX,
            locus_id: None,
            phenotypes: Some(vec!["bmi".into(), "depression".into()]),
            target,
            variances: false,
            adap_thresh: Some(vec![1e-4, 1e-6]),
            p_values: true,
            cis: true,
            only_full_model: false,
            max_r2: default_max_r2(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_lava_contract() {
        let spec = spec(LavaAnalysis::Bivar, None);
        let container = container_spec(&spec).unwrap();
        assert_eq!(container.image, LAVA_ORIGINAL_IMAGE);
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, LAVA_TUTORIAL_REF_PANEL);
        let script = container.script.as_deref().unwrap();
        assert!(script.contains("LAVA::process.input"));
        assert!(script.contains("LAVA::process.locus"));
        assert!(script.contains("LAVA::run.bivar"));
        assert!(script.contains("\"/panels/lava_ref/g1000_test\""));
        assert!(script.contains("utils::unzip"));
        assert!(script.contains("analysis = \"bivar\""));
        assert_eq!(container.outputs.len(), 3);
    }

    #[test]
    fn default_binding_uses_ukb_panel() {
        assert_eq!(default_panel_bindings()[0].bundle_id, LAVA_UKB_EUR_PANEL);
    }

    #[test]
    fn validates_analysis_target_contract() {
        assert!(validate(&spec(LavaAnalysis::Univ, None)).is_ok());
        assert!(validate(&spec(LavaAnalysis::Univ, Some(vec!["bmi".into()]))).is_err());
        assert!(
            validate(&spec(
                LavaAnalysis::Pcor,
                Some(vec!["bmi".into(), "depression".into()])
            ))
            .is_ok()
        );
        assert!(
            validate(&spec(
                LavaAnalysis::Pcor,
                Some(vec!["bmi".into(), "bmi".into()])
            ))
            .is_err()
        );
        assert!(validate(&spec(LavaAnalysis::Multireg, None)).is_err());
    }

    #[test]
    fn generates_each_official_analysis_call() {
        let pcor = container_spec(&spec(
            LavaAnalysis::Pcor,
            Some(vec!["depression".into(), "neuro".into()]),
        ))
        .unwrap();
        assert!(pcor.script.as_deref().unwrap().contains(
            "LAVA::run.pcor(locus, phenos = phenos, target = c(\"depression\", \"neuro\")"
        ));

        let multireg =
            container_spec(&spec(LavaAnalysis::Multireg, Some(vec!["bmi".into()]))).unwrap();
        assert!(
            multireg
                .script
                .as_deref()
                .unwrap()
                .contains("LAVA::run.multireg(locus, phenos = phenos, target = c(\"bmi\")")
        );
    }

    #[test]
    fn rejects_unsafe_bundle_paths() {
        let mut value = spec(LavaAnalysis::Univ, None);
        value.input_info_file = "/etc/passwd".into();
        assert!(validate(&value).is_err());
        value.input_info_file = "../input.info.txt".into();
        assert!(validate(&value).is_err());
    }
}
