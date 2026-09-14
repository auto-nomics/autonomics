//! Containerized two-sample MR node backed by the official R package.
//!
//! The wrapper accepts one SNP-merged exposure/outcome summary-statistics
//! File. It selects independent instruments with official PLINK2 against the
//! catalog-backed 1000G EUR binary reference, then delegates all harmonisation
//! and causal estimates to `TwoSampleMR`.

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
use crate::plink2_clump_container::PLINK2_REF_BINARY_PANEL;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const TWOSAMPLEMR_CONTAINER_KIND: &str = "twosamplemr_container";
pub const TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY: &str = "twosamplemr";
pub const TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:c270de9978906ee48cbba2ac484ba3df9e86dddc69efd908c6f908963114002a";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/twosamplemr_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_CLUMP_P1: f64 = 5e-8;
const DEFAULT_CLUMP_P2: f64 = 1e-6;
const DEFAULT_CLUMP_R2: f64 = 0.001;
const DEFAULT_CLUMP_KB: u32 = 10_000;

const SUPPORTED_METHODS: &[&str] = &[
    "mr_wald_ratio",
    "mr_two_sample_ml",
    "mr_egger_regression",
    "mr_egger_regression_bootstrap",
    "mr_simple_median",
    "mr_weighted_median",
    "mr_penalised_weighted_median",
    "mr_ivw",
    "mr_ivw_radial",
    "mr_ivw_mre",
    "mr_ivw_fe",
    "mr_simple_mode",
    "mr_weighted_mode",
    "mr_weighted_mode_nome",
    "mr_simple_mode_nome",
    "mr_sign",
    "mr_uwr",
    "mr_grip",
];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TwoSampleMrContainerSpec {
    pub id_exposure: String,
    pub exposure: String,
    pub id_outcome: String,
    pub outcome: String,
    #[serde(default = "default_method_list")]
    pub method_list: Vec<String>,
    #[serde(default = "default_harmonise_action")]
    pub harmonise_action: u8,
    #[serde(default = "default_true")]
    pub clump: bool,
    #[serde(default = "default_clump_p1")]
    pub clump_p1: f64,
    #[serde(default = "default_clump_p2")]
    pub clump_p2: f64,
    #[serde(default = "default_clump_r2")]
    pub clump_r2: f64,
    #[serde(default = "default_clump_kb")]
    pub clump_kb: u32,
    #[serde(default)]
    pub chr: Option<u8>,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_harmonise_action() -> u8 {
    2
}

fn default_clump_p1() -> f64 {
    DEFAULT_CLUMP_P1
}

fn default_clump_p2() -> f64 {
    DEFAULT_CLUMP_P2
}

fn default_clump_r2() -> f64 {
    DEFAULT_CLUMP_R2
}

fn default_clump_kb() -> u32 {
    DEFAULT_CLUMP_KB
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_method_list() -> Vec<String> {
    [
        "mr_egger_regression",
        "mr_weighted_median",
        "mr_ivw",
        "mr_simple_mode",
        "mr_weighted_mode",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub struct TwoSampleMrContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl TwoSampleMrContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct TwoSampleMrContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for TwoSampleMrContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for TwoSampleMrContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        TWOSAMPLEMR_CONTAINER_KIND
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

pub fn validate(spec: &TwoSampleMrContainerSpec) -> Result<(), String> {
    for (name, value) in [
        ("id_exposure", &spec.id_exposure),
        ("exposure", &spec.exposure),
        ("id_outcome", &spec.id_outcome),
        ("outcome", &spec.outcome),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
    }
    if spec.method_list.is_empty() {
        return Err("method_list cannot be empty".into());
    }
    for method in &spec.method_list {
        if !SUPPORTED_METHODS.contains(&method.as_str()) {
            return Err(format!("unsupported TwoSampleMR method `{method}`"));
        }
    }
    if !(1..=3).contains(&spec.harmonise_action) {
        return Err("harmonise_action must be 1, 2, or 3".into());
    }
    if spec.clump_p1 <= 0.0
        || spec.clump_p1 > 1.0
        || !spec.clump_p1.is_finite()
        || spec.clump_p2 <= 0.0
        || spec.clump_p2 > 1.0
        || !spec.clump_p2.is_finite()
        || spec.clump_r2 <= 0.0
        || spec.clump_r2 > 1.0
        || !spec.clump_r2.is_finite()
    {
        return Err("clump thresholds and clump_r2 must be finite and not exceed 1".into());
    }
    if spec.clump_p2 < spec.clump_p1 {
        return Err("clump_p2 must be greater than or equal to clump_p1".into());
    }
    if spec.clump_kb == 0 {
        return Err("clump_kb must be greater than zero".into());
    }
    if let Some(chr) = spec.chr
        && !(1..=22).contains(&chr)
    {
        return Err("chr must lie in 1..=22 when specified".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
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

fn build_script(spec: &TwoSampleMrContainerSpec) -> String {
    let (sequence, chromosome_argument) = match spec.chr {
        Some(chr) => (format!("{chr} {chr}"), format!("--chr {chr} \\\n")),
        None => ("1 22".into(), String::new()),
    };
    let clump_block = if spec.clump {
        format!(
            "Rscript --vanilla -e 'data <- read.delim(Sys.getenv(\"AUTONOMICS_INPUT0\"), check.names = FALSE, stringsAsFactors = FALSE); p <- if (is.null(data$pval_exposure)) 2 * pnorm(-abs(data$beta_exposure / data$se_exposure)) else data$pval_exposure; write.table(data.frame(SNP = data$snp, P = p), \"/work/clump_input.tsv\", sep = \"\\t\", quote = FALSE, row.names = FALSE)'\n\
             mkdir -p /work/per_chr\n\
             : > /work/all.clumps\n\
             for chr in $(seq {sequence}); do\n\
             \x20 out=/work/per_chr/chr${{chr}}\n\
             \x20 plink2 --bfile /panels/plink_ref/1000G.EUR.QC.${{chr}} \\\n\
             \x20\x20\x20 --clump /work/clump_input.tsv \\\n\
             \x20\x20\x20 --clump-id-field SNP --clump-p-field P \\\n\
             \x20\x20\x20 {chromosome_argument}\
             \x20\x20\x20 --clump-p1 {clump_p1:.0e} --clump-p2 {clump_p2:.0e} \\\n\
             \x20\x20\x20 --clump-r2 {clump_r2} --clump-kb {clump_kb} --threads 1 \\\n\
             \x20\x20\x20 --out \"$out\" >>\"${{AUTONOMICS_OUTPUT3}}\" 2>&1\n\
             \x20 if [ -f \"$out.clumps\" ]; then cat \"$out.clumps\" >> /work/all.clumps; fi\n\
             done\n",
            clump_p1 = spec.clump_p1,
            clump_p2 = spec.clump_p2,
            clump_r2 = spec.clump_r2,
            clump_kb = spec.clump_kb,
        )
    } else {
        String::new()
    };
    let clump_selection = if spec.clump {
        "clumps <- read.delim(\"/work/all.clumps\", check.names = FALSE, stringsAsFactors = FALSE)\n\
         \x20 selected <- unique(clumps$ID)\n\
         \x20 exposure_dat <- exposure_dat[exposure_dat$SNP %in% selected, , drop = FALSE]\n"
    } else {
        "selected <- unique(exposure_dat$SNP)\n"
    };

    let r_script = format!(
        r#"
input <- Sys.getenv("AUTONOMICS_INPUT0")
table_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
harmonised_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
result_path <- Sys.getenv("AUTONOMICS_OUTPUT2")
log_path <- Sys.getenv("AUTONOMICS_OUTPUT3")
data <- read.delim(input, check.names = FALSE, stringsAsFactors = FALSE)
pval <- if (is.null(data$pval_exposure)) 2 * pnorm(-abs(data$beta_exposure / data$se_exposure)) else data$pval_exposure
exposure_dat <- data.frame(SNP = data$snp, beta.exposure = data$beta_exposure, se.exposure = data$se_exposure, effect_allele.exposure = data$effect_allele_exposure, other_allele.exposure = data$other_allele_exposure, eaf.exposure = data$eaf_exposure, pval.exposure = pval, id.exposure = {id_exposure}, exposure = {exposure})
outcome_dat <- data.frame(SNP = data$snp, beta.outcome = data$beta_outcome, se.outcome = data$se_outcome, effect_allele.outcome = data$effect_allele_outcome, other_allele.outcome = data$other_allele_outcome, eaf.outcome = data$eaf_outcome, id.outcome = {id_outcome}, outcome = {outcome})
{clump_selection}sink(log_path, split = TRUE, append = TRUE)
cat("TwoSampleMR:", as.character(packageVersion("TwoSampleMR")), "\n")
cat("Selected instruments:", length(unique(exposure_dat$SNP)), "\n")
harmonised <- TwoSampleMR::harmonise_data(exposure_dat, outcome_dat, action = {action})
estimates <- TwoSampleMR::mr(harmonised, method_list = {methods})
print(estimates)
sink()
write.table(estimates, table_path, sep = "\t", quote = FALSE, row.names = FALSE)
write.table(harmonised, harmonised_path, sep = "\t", quote = FALSE, row.names = FALSE)
result <- list(exposure_dat = exposure_dat, outcome_dat = outcome_dat, harmonised = harmonised, mr = estimates)
saveRDS(result, result_path)
RSCRIPT
"#,
        id_exposure = r_string(&spec.id_exposure),
        exposure = r_string(&spec.exposure),
        id_outcome = r_string(&spec.id_outcome),
        outcome = r_string(&spec.outcome),
        action = spec.harmonise_action,
        methods = r_strings(&spec.method_list),
    );

    format!(
        "set -eu\n: > \"${{AUTONOMICS_OUTPUT3}}\"\n{clump_block}Rscript --vanilla - <<'RSCRIPT'\n{r_script}",
        r_script = r_script.trim_start()
    )
}

pub fn container_spec(spec: &TwoSampleMrContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: acr_image(
            TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY,
            TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["sh".into(), "-c".into()],
        script: Some(build_script(spec)),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "twosamplemr_results.tsv".into(),
                format: Some("tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "twosamplemr_harmonised.tsv".into(),
                format: Some("tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "twosamplemr.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "twosamplemr.log".into(),
                format: Some("twosamplemr_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: PLINK2_REF_BINARY_PANEL.into(),
            mount_path: "/panels/plink_ref".into(),
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

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("plink_ref", PLINK2_REF_BINARY_PANEL)]
}

impl NodeFactory for TwoSampleMrContainerNodeFactory {
    fn kind(&self) -> &'static str {
        TWOSAMPLEMR_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official TwoSampleMR in an ephemeral OCI container."
    }

    fn doc(&self) -> &'static str {
        "Runs the official R TwoSampleMR 0.7.9 package. Input is one \
        SNP-merged tab-separated File with exposure and outcome effects, SEs, \
        alleles, EAF, and optional exposure p-values. Instruments are clumped \
        with official PLINK2 against the 1000G EUR catalog panel; then \
        TwoSampleMR harmonises alleles and runs the selected methods. The \
        wrapper publishes the official MR table, harmonised table, full RDS \
        result, and execution log."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TwoSampleMrContainerSpec)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        Ok(panel_bindings())
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: TwoSampleMrContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(TwoSampleMrContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: TwoSampleMrContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TwoSampleMrContainerSpec {
        TwoSampleMrContainerSpec {
            id_exposure: "ieu-a-2".into(),
            exposure: "Body mass index".into(),
            id_outcome: "ieu-a-7".into(),
            outcome: "Coronary heart disease".into(),
            method_list: default_method_list(),
            harmonise_action: default_harmonise_action(),
            clump: true,
            clump_p1: default_clump_p1(),
            clump_p2: default_clump_p2(),
            clump_r2: default_clump_r2(),
            clump_kb: default_clump_kb(),
            chr: Some(22),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_twosamplemr_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            acr_image(
                TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY,
                TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        let script = container.script.unwrap();
        assert!(script.contains("TwoSampleMR::harmonise_data"));
        assert!(script.contains("TwoSampleMR::mr"));
        assert!(script.contains("plink2 --bfile"));
        assert!(script.ends_with("RSCRIPT\n"));
        assert_eq!(container.outputs.len(), 4);
    }

    #[test]
    fn rejects_unsupported_method_and_bad_thresholds() {
        let mut value = spec();
        value.method_list = vec!["mr_presso".into()];
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.method_list = default_method_list();
        value.clump_p2 = 1e-9;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn renders_preclumped_input_without_plink_call() {
        let mut value = spec();
        value.clump = false;
        let container = container_spec(&value).unwrap();
        let script = container.script.unwrap();
        assert!(!script.contains("plink2 --bfile"));
        assert!(script.contains("TwoSampleMR::harmonise_data"));
        assert!(script.ends_with("RSCRIPT\n"));
    }
}
