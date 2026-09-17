//! Containerized LAVA scan backed by the official R package.
//!
//! This follows the official multiple-locus workflow: process the input once,
//! iterate the user-provided loci table, and run `run.univ.bivar` for each
//! processable locus. The LD reference binding is identical to `lava_container`.

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
use crate::image_registry::acr_image;
use crate::lava_container;
use crate::lava_container::{
    LAVA_ORIGINAL_IMAGE_DIGEST, LAVA_ORIGINAL_IMAGE_REPOSITORY, LAVA_TUTORIAL_REF_PANEL,
    LAVA_UKB_EUR_PANEL,
};
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const LAVA_SCAN_CONTAINER_KIND: &str = "lava_scan_container";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/lava_scan_container";
const DEFAULT_TIMEOUT_SECS: u64 = 86_400;
const DEFAULT_MIN_K: usize = 2;
const DEFAULT_PRUNE_THRESH: f64 = 99.0;
const DEFAULT_MAX_PROP_K: f64 = 0.75;
const DEFAULT_MAX_BLOCK_SIZE: usize = 3000;
const DEFAULT_UNIV_THRESHOLD: f64 = 0.05;
const DEFAULT_PARAM_LIM: f64 = 1.25;

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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LavaScanContainerSpec {
    /// Catalog panel ID whose LD reference the container should mount.
    pub panel_id: String,
    /// Optional override for the panel mount path inside the container.
    #[serde(default)]
    pub mount_path_override: Option<String>,
    /// Optional override for the filename prefix inside the mounted panel.
    #[serde(default)]
    pub ref_prefix_override: Option<String>,
    /// Whether a sample-overlap matrix is connected to input port 2.
    #[serde(default)]
    pub sample_overlap: bool,
    /// Phenotypes in the same order as their sumstats input ports.
    pub phenotypes: Vec<String>,
    /// Optional target phenotype for `run.univ.bivar`; null tests all
    /// phenotype pairs that pass the univariate threshold.
    #[serde(default)]
    pub target: Option<Vec<String>>,
    /// Optional `LOC` IDs to scan. All rows in the loci table are scanned when
    /// this field is omitted.
    #[serde(default)]
    pub locus_ids: Option<Vec<String>>,
    /// Optional chromosome filter. LAVA uses 23 for chromosome X.
    #[serde(default)]
    pub chr: Option<u8>,
    /// Univariate significance threshold used before running bivariate tests.
    #[serde(default = "default_univ_threshold")]
    pub univ_threshold: f64,
    /// Adaptive p-value thresholds. `null` uses the official default.
    #[serde(default)]
    pub adap_thresh: Option<Vec<f64>>,
    /// Compute simulation p-values.
    #[serde(default = "default_true")]
    pub p_values: bool,
    /// Compute confidence intervals.
    #[serde(default = "default_true")]
    pub cis: bool,
    /// VFS prefix for immutable LAVA scan artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum runtime for the complete scan.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_univ_threshold() -> f64 {
    DEFAULT_UNIV_THRESHOLD
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

pub struct LavaScanContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl LavaScanContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct LavaScanContainerNode {
    inner: Box<dyn DagNode>,
    ports: NodePorts,
}

impl Clone for LavaScanContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
            ports: self.ports.clone(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for LavaScanContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LAVA_SCAN_CONTAINER_KIND
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

fn r_strings(values: &[String]) -> String {
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

pub fn validate(spec: &LavaScanContainerSpec) -> Result<(), String> {
    if spec.panel_id.trim().is_empty() {
        return Err("panel_id cannot be empty".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if spec.phenotypes.is_empty() || spec.phenotypes.iter().any(|value| value.trim().is_empty()) {
        return Err("phenotypes cannot be empty and cannot contain empty IDs".into());
    }
    let unique_phenotypes = spec
        .phenotypes
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if unique_phenotypes.len() != spec.phenotypes.len() {
        return Err("phenotypes cannot contain duplicate IDs".into());
    }
    if let Some(target) = &spec.target {
        if target.len() != 1 || !spec.phenotypes.contains(&target[0]) {
            return Err("target must contain exactly one configured phenotype".into());
        }
    }
    if let Some(ids) = &spec.locus_ids {
        if ids.is_empty() || ids.iter().any(|id| id.trim().is_empty()) {
            return Err("locus_ids cannot be empty when provided".into());
        }
        if ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len() {
            return Err("locus_ids cannot contain duplicates".into());
        }
    }
    if let Some(chr) = spec.chr
        && !(1..=23).contains(&chr)
    {
        return Err("chr must be between 1 and 23 (23 is chromosome X)".into());
    }
    if !spec.univ_threshold.is_finite()
        || !(0.0..=1.0).contains(&spec.univ_threshold)
        || spec.univ_threshold == 0.0
    {
        return Err("univ_threshold must be finite and in (0, 1]".into());
    }
    if let Some(thresholds) = &spec.adap_thresh {
        if thresholds.is_empty() {
            return Err("adap_thresh cannot be empty when provided".into());
        }
        for threshold in thresholds {
            if !threshold.is_finite() || *threshold <= 0.0 {
                return Err("adap_thresh values must be finite and greater than zero".into());
            }
        }
    }
    Ok(())
}

fn build_script(spec: &LavaScanContainerSpec) -> String {
    let target = spec
        .target
        .as_ref()
        .and_then(|values| values.first())
        .map(|value| r_string(value))
        .unwrap_or_else(|| "NULL".into());
    let locus_ids = spec
        .locus_ids
        .as_ref()
        .map(|values| r_strings(values))
        .unwrap_or_else(|| "NULL".into());
    let chr = spec
        .chr
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".into());
    let sample_overlap = if spec.sample_overlap {
        "Sys.getenv(\"AUTONOMICS_INPUT2\")".to_string()
    } else {
        "NULL".into()
    };
    let sumstats_base = if spec.sample_overlap { 3 } else { 2 };
    let sumstats = spec
        .phenotypes
        .iter()
        .enumerate()
        .map(|(index, _)| format!("Sys.getenv(\"AUTONOMICS_INPUT{}\")", sumstats_base + index))
        .collect::<Vec<_>>()
        .join(", ");
    let expected_inputs = sumstats_base + spec.phenotypes.len();

    format!(
        r#"options(width = 200)
sink(Sys.getenv("AUTONOMICS_OUTPUT3"), split = TRUE)
on.exit(sink(), add = TRUE)
if (as.integer(Sys.getenv("AUTONOMICS_INPUT_COUNT")) != {expected_inputs}) {{
  stop("LAVA scan input port count does not match its specification")
}}

phenos <- {phenotypes}
input_info <- read.table(
  Sys.getenv("AUTONOMICS_INPUT0"),
  header = TRUE,
  check.names = FALSE,
  stringsAsFactors = FALSE
)
if (!all(c("phenotype", "cases", "controls") %in% names(input_info))) {{
  stop("input.info must contain phenotype, cases, and controls columns")
}}
if (anyDuplicated(input_info$phenotype)) {{
  stop("input.info phenotype IDs must be unique")
}}
if (!all(phenos %in% input_info$phenotype)) {{
  stop("one or more configured phenotypes are missing from input.info")
}}
input_info <- input_info[match(phenos, input_info$phenotype), , drop = FALSE]
input_info$filename <- unname(c({sumstats}))
normalized_input_info <- file.path(
  Sys.getenv("AUTONOMICS_WORKDIR"),
  ".autonomics",
  "lava_scan",
  "input.info.txt"
)
dir.create(dirname(normalized_input_info), recursive = TRUE, showWarnings = FALSE)
write.table(
  input_info,
  normalized_input_info,
  sep = "\t",
  quote = FALSE,
  row.names = FALSE
)

input <- LAVA::process.input(
  input.info.file = normalized_input_info,
  sample.overlap.file = {sample_overlap},
  ref.prefix = "{mount_path}/{ref_prefix}",
  phenos = phenos
)
loci <- LAVA::read.loci(Sys.getenv("AUTONOMICS_INPUT1"))
if (anyDuplicated(loci$LOC)) {{
  stop("loci table contains duplicate LOC IDs")
}}

requested_locus_ids <- {locus_ids}
if (!is.null(requested_locus_ids)) {{
  matches <- match(requested_locus_ids, as.character(loci$LOC))
  if (any(is.na(matches))) {{
    stop("requested loci are absent: ", paste(requested_locus_ids[is.na(matches)], collapse = ", "))
  }}
  loci <- loci[matches, , drop = FALSE]
}}
selected_chr <- {chr}
if (!is.null(selected_chr)) {{
  loci <- loci[loci$CHR == selected_chr, , drop = FALSE]
}}
if (nrow(loci) == 0) {{
  stop("loci selection is empty")
}}

empty_result <- function() {{
  data.frame(
    locus = character(),
    chr = integer(),
    start = integer(),
    stop = integer(),
    n.snps = integer(),
    n.pcs = integer(),
    stringsAsFactors = FALSE
  )
}}

univ_results <- list()
bivar_results <- list()
raw_results <- list()
failed <- 0L
skipped <- 0L
cat("Starting official LAVA scan for", nrow(loci), "loci\n")
for (row_index in seq_len(nrow(loci))) {{
  locus_row <- loci[row_index, , drop = FALSE]
  locus_id <- as.character(locus_row$LOC)
  cat("Processing locus", locus_id, "\n")
  locus <- tryCatch(
    LAVA::process.locus(
      locus_row,
      input,
      phenos = phenos,
      min.K = {min_k},
      prune.thresh = {prune_thresh},
      max.prop.K = {max_prop_k},
      drop.failed = TRUE,
      max.block.size = {max_block_size},
      cap.estimates = TRUE
    ),
    error = function(error) {{
      failed <<- failed + 1L
      message <- paste0("Error processing locus ", locus_id, ": ",
                        conditionMessage(error))
      cat(message, "\n")
      NULL
    }}
  )
  if (is.null(locus)) {{
    skipped <<- skipped + 1L
    next
  }}

  result <- tryCatch(
    LAVA::run.univ.bivar(
      locus,
      phenos = phenos,
      target = {target},
      univ.thresh = {univ_threshold},
      adap.thresh = {adap_thresh},
      p.values = {p_values},
      CIs = {cis},
      param.lim = {param_lim},
      cap.estimates = TRUE
    ),
    error = function(error) {{
      failed <<- failed + 1L
      message <- paste0("Error analyzing locus ", locus_id, ": ",
                        conditionMessage(error))
      cat(message, "\n")
      NULL
    }}
  )
  if (is.null(result)) {{
    skipped <<- skipped + 1L
    next
  }}

  raw_results[[locus_id]] <- result
  locus_info <- data.frame(
    locus = locus$id,
    chr = locus$chr,
    start = locus$start,
    stop = locus$stop,
    n.snps = locus$n.snps,
    n.pcs = locus$K,
    stringsAsFactors = FALSE
  )
  if (!is.null(result$univ)) {{
    univ_results[[length(univ_results) + 1L]] <- cbind(locus_info, result$univ)
  }}
  if (!is.null(result$bivar)) {{
    bivar_results[[length(bivar_results) + 1L]] <- cbind(locus_info, result$bivar)
  }}
}}

univ <- if (length(univ_results)) do.call(rbind, univ_results) else empty_result()
bivar <- if (length(bivar_results)) do.call(rbind, bivar_results) else empty_result()
cat(
  "Finished official LAVA scan:",
  nrow(loci), "requested;",
  nrow(univ), "univariate rows;",
  nrow(bivar), "bivariate rows;",
  skipped, "skipped;",
  failed, "failed\n"
)
write.table(univ, Sys.getenv("AUTONOMICS_OUTPUT0"), sep = "\t", quote = FALSE, row.names = FALSE)
write.table(bivar, Sys.getenv("AUTONOMICS_OUTPUT1"), sep = "\t", quote = FALSE, row.names = FALSE)
saveRDS(
  list(loci = loci, univ = univ, bivar = bivar, raw_results = raw_results),
  Sys.getenv("AUTONOMICS_OUTPUT2")
)
"#,
        phenotypes = r_strings(&spec.phenotypes),
        sumstats = sumstats,
        sample_overlap = sample_overlap,
        mount_path = spec.mount_path_override.clone().unwrap_or_else(|| {
            resolve_panel(&spec.panel_id)
                .map(|panel| panel.mount_path.to_string())
                .unwrap_or_default()
        }),
        ref_prefix = spec.ref_prefix_override.clone().unwrap_or_else(|| {
            resolve_panel(&spec.panel_id)
                .map(|panel| panel.ref_prefix.to_string())
                .unwrap_or_default()
        }),
        locus_ids = locus_ids,
        chr = chr,
        min_k = DEFAULT_MIN_K,
        prune_thresh = DEFAULT_PRUNE_THRESH,
        max_prop_k = DEFAULT_MAX_PROP_K,
        max_block_size = DEFAULT_MAX_BLOCK_SIZE,
        target = target,
        univ_threshold = spec.univ_threshold,
        adap_thresh = r_adap_thresh(&spec.adap_thresh),
        p_values = r_bool(spec.p_values),
        cis = r_bool(spec.cis),
        param_lim = DEFAULT_PARAM_LIM,
    )
}

pub fn container_spec(spec: &LavaScanContainerSpec) -> Result<ContainerCommandSpec, String> {
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

    Ok(ContainerCommandSpec {
        image: acr_image(LAVA_ORIGINAL_IMAGE_REPOSITORY, LAVA_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["Rscript".into()],
        script: Some(build_script(spec)),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "lava_scan.univ.tsv".into(),
                format: Some("lava_univ_result_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "lava_scan.bivar.tsv".into(),
                format: Some("lava_bivar_result_tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "lava_scan.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "lava_scan.log".into(),
                format: Some("lava_scan_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: spec.panel_id.clone(),
            mount_path,
        }],
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

fn port_layout_for(sample_overlap: bool, phenotype_count: usize) -> NodePorts {
    let mut ports = NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_input_port_of_type(None, PortType::File);
    if sample_overlap {
        ports = ports.add_input_port_of_type(None, PortType::File);
    }
    for _ in 0..phenotype_count {
        ports = ports.add_input_port_of_type(None, PortType::File);
    }
    for _ in 0..4 {
        ports = ports.add_output_port_of_type(None, PortType::File);
    }
    ports
}

fn panel_bindings_for(panel_id: &str) -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("lava_ref", panel_id)]
}

impl NodeFactory for LavaScanContainerNodeFactory {
    fn kind(&self) -> &'static str {
        LAVA_SCAN_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs the official LAVA multiple-locus scan in Podman."
    }

    fn doc(&self) -> &'static str {
        "Runs the official LAVA multiple-locus workflow over a user-provided \
loci table. Input port 0 is input-info, port 1 is the loci table, optional \
port 2 is sample-overlap, and the remaining ports are sumstats Files in \
phenotype order. The node calls `process.input` once, iterates `read.loci`, \
calls `process.locus` and `run.univ.bivar` per selected locus, and emits \
combined univariate/bivariate TSVs, an RDS result object, and the complete \
log. `locus_ids` and `chr` can subset the scan; locus failures are logged and \
skipped. The panel binding is identical to `lava_container`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaScanContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        lava_container::default_panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let parsed: LavaScanContainerSpec =
            serde_json::from_value(spec).map_err(dag_core::registry::error::Error::from)?;
        Ok(panel_bindings_for(&parsed.panel_id))
    }

    fn ports(&self) -> NodePorts {
        port_layout_for(false, 1)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LavaScanContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = container_spec
            .panel_bundles
            .iter()
            .map(|_| node_ctx.bound_data_bundle("lava_ref").cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            Arc::clone(&self.runtime) as Arc<dyn PodmanConnection>,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(LavaScanContainerNode {
            inner: Box::new(node),
            ports: port_layout_for(spec.sample_overlap, spec.phenotypes.len()),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: LavaScanContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout_for(spec.sample_overlap, spec.phenotypes.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LavaScanContainerSpec {
        LavaScanContainerSpec {
            panel_id: LAVA_TUTORIAL_REF_PANEL.into(),
            mount_path_override: None,
            ref_prefix_override: None,
            sample_overlap: true,
            phenotypes: vec!["depression".into(), "neuro".into(), "bmi".into()],
            target: None,
            locus_ids: Some(vec!["100".into(), "230".into()]),
            chr: None,
            univ_threshold: default_univ_threshold(),
            adap_thresh: Some(vec![1e-4, 1e-6]),
            p_values: true,
            cis: true,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_lava_scan_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(LAVA_ORIGINAL_IMAGE_REPOSITORY, LAVA_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, LAVA_TUTORIAL_REF_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/lava_ref");
        assert_eq!(container.outputs.len(), 4);

        let script = container.script.as_deref().unwrap();
        assert_eq!(script.matches("LAVA::process.input").count(), 1);
        assert!(script.contains("requested_locus_ids <- c(\"100\", \"230\")"));
        assert!(script.contains("for (row_index in seq_len(nrow(loci)))"));
        assert!(script.contains("LAVA::process.locus"));
        assert!(script.contains("LAVA::run.univ.bivar"));
        assert!(script.contains("univ.thresh = 0.05"));
        assert!(script.contains("Sys.getenv(\"AUTONOMICS_OUTPUT0\")"));
    }

    #[test]
    fn scan_panel_binding_matches_single_locus_container() {
        assert_eq!(
            panel_bindings_for(LAVA_UKB_EUR_PANEL),
            vec![DataBundleBinding::new("lava_ref", LAVA_UKB_EUR_PANEL)]
        );
    }

    #[test]
    fn validates_scan_selection_and_analysis_parameters() {
        assert!(validate(&spec()).is_ok());

        let mut value = spec();
        value.locus_ids = Some(Vec::new());
        assert!(validate(&value).is_err());

        value = spec();
        value.chr = Some(24);
        assert!(validate(&value).is_err());

        value = spec();
        value.target = Some(Vec::new());
        assert!(validate(&value).is_err());

        value = spec();
        value.univ_threshold = 0.0;
        assert!(validate(&value).is_err());
    }
}
