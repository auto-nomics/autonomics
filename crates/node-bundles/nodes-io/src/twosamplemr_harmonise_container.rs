//! Two-file TwoSampleMR harmoniser node backed by the official R package.
//!
//! Accepts separate exposure and outcome summary-statistics files, runs the
//! canonical [`TwoSampleMR::read_exposure_data`] + [`TwoSampleMR::read_outcome_data`]
//! + [`TwoSampleMR::harmonise_data`] pipeline inside the same pinned official
//! image used by [`crate::twosamplemr_container`], and writes the merged
//! tab-separated summary-statistics file that [`crate::twosamplemr_container`]
//! already expects on its input port. The wrapper's column contract is
//! snake_case (`snp`, `beta_exposure`, `se_exposure`, …), so the R script
//! rewrites TwoSampleMR's native dotted internal names (`SNP`, `beta.exposure`,
//! …) to snake_case before writing the harmonised table.

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
use crate::twosamplemr_container::{
    TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST, TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY,
};
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const TWOSAMPLEMR_HARMONISE_CONTAINER_KIND: &str = "twosamplemr_harmonise_container";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/twosamplemr_harmonise";
const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TwoSampleMrHarmoniseContainerSpec {
    pub id_exposure: String,
    pub exposure: String,
    pub id_outcome: String,
    pub outcome: String,
    #[serde(default = "default_harmonise_action")]
    pub harmonise_action: u8,
    /// Optional free-text units label stamped onto `units.exposure` in the
    /// harmonised table. Leave unset to keep whatever `read_exposure_data`
    /// found (NA when the source file has no units column).
    #[serde(default)]
    pub units_exposure: Option<String>,
    /// Optional `units.outcome` label; same semantics as `units_exposure`.
    #[serde(default)]
    pub units_outcome: Option<String>,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_harmonise_action() -> u8 {
    2
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct TwoSampleMrHarmoniseContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl TwoSampleMrHarmoniseContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct TwoSampleMrHarmoniseContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for TwoSampleMrHarmoniseContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for TwoSampleMrHarmoniseContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        TWOSAMPLEMR_HARMONISE_CONTAINER_KIND
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

pub fn validate(spec: &TwoSampleMrHarmoniseContainerSpec) -> Result<(), String> {
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
    if !(1..=3).contains(&spec.harmonise_action) {
        return Err("harmonise_action must be 1, 2, or 3".into());
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

fn build_script(spec: &TwoSampleMrHarmoniseContainerSpec) -> String {
    let units_exposure = match &spec.units_exposure {
        Some(units) => format!("exp$units.exposure <- {}", r_string(units)),
        None => String::new(),
    };
    let units_outcome = match &spec.units_outcome {
        Some(units) => format!("out$units.outcome <- {}", r_string(units)),
        None => String::new(),
    };
    format!(
        r#"exp_path <- Sys.getenv("AUTONOMICS_INPUT0")
out_path <- Sys.getenv("AUTONOMICS_INPUT1")
harm_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
log_path <- Sys.getenv("AUTONOMICS_OUTPUT1")
sink(log_path, split = TRUE)
cat("TwoSampleMR:", as.character(packageVersion("TwoSampleMR")), "\n")
exp <- TwoSampleMR::read_exposure_data(exp_path)
out <- TwoSampleMR::read_outcome_data(out_path)
cat("exposure rows:", nrow(exp), " columns:", paste(names(exp), collapse = ","), "\n")
cat("outcome rows:", nrow(out), " columns:", paste(names(out), collapse = ","), "\n")
exp$id.exposure <- {id_exposure}
exp$exposure <- {exposure}
{units_exposure}
out$id.outcome <- {id_outcome}
out$outcome <- {outcome}
{units_outcome}
harm <- TwoSampleMR::harmonise_data(exp, out, action = {action})
cat("harmonised rows:", nrow(harm), " columns:", paste(names(harm), collapse = ","), "\n")
if (nrow(harm) == 0) stop("harmonise_data produced 0 rows; check SNP overlap between exposure and outcome")
names(harm)[names(harm) == "SNP"] <- "snp"
names(harm) <- gsub("\\.", "_", names(harm))
write.table(harm, harm_path, sep = "\t", quote = FALSE, row.names = FALSE)
sink()
"#,
        id_exposure = r_string(&spec.id_exposure),
        exposure = r_string(&spec.exposure),
        id_outcome = r_string(&spec.id_outcome),
        outcome = r_string(&spec.outcome),
        action = spec.harmonise_action,
        units_exposure = units_exposure,
        units_outcome = units_outcome,
    )
}

pub fn container_spec(
    spec: &TwoSampleMrHarmoniseContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    Ok(ContainerCommandSpec {
        image: registry_image(
            TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY,
            TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST,
        )?,
        command: vec!["Rscript".into()],
        script: Some(build_script(spec)),
        files: Default::default(),
        env: Default::default(),
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "harmonised.tsv".into(),
                format: Some("tsv".into()),
            },
            ContainerCommandOutputSpec {
                path: "harmonise.log".into(),
                format: Some("twosamplemr_log".into()),
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
        .add_input_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for TwoSampleMrHarmoniseContainerNodeFactory {
    fn kind(&self) -> &'static str {
        TWOSAMPLEMR_HARMONISE_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Harmonises exposure and outcome sumstats via official TwoSampleMR, \
         emitting the merged snake_case TSV that `twosamplemr_container` accepts."
    }

    fn doc(&self) -> &'static str {
        "Two-file harmoniser. Input port 0 is an exposure summary-statistics \
         TSV (any layout `TwoSampleMR::read_exposure_data` can parse); input \
         port 1 is an outcome summary-statistics TSV (any layout \
         `TwoSampleMR::read_outcome_data` can parse). The wrapper calls \
         `TwoSampleMR::harmonise_data` and rewrites the native dotted column \
         names to snake_case so the output is directly consumable by \
         `twosamplemr_container`. Output port 0 is the merged TSV; output \
         port 1 is the run log."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TwoSampleMrHarmoniseContainerSpec)
    }

    fn data_bundles(&self) -> Vec<dag_core::DataBundleBinding> {
        Vec::new()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<dag_core::DataBundleBinding>> {
        Ok(Vec::new())
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: TwoSampleMrHarmoniseContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            self.kind(),
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            Vec::new(),
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(TwoSampleMrHarmoniseContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: TwoSampleMrHarmoniseContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TwoSampleMrHarmoniseContainerSpec {
        TwoSampleMrHarmoniseContainerSpec {
            id_exposure: "ieu-a-2".into(),
            exposure: "Body mass index".into(),
            id_outcome: "ieu-a-7".into(),
            outcome: "Coronary heart disease".into(),
            harmonise_action: default_harmonise_action(),
            units_exposure: None,
            units_outcome: None,
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
        }
    }

    #[test]
    fn builds_official_harmonise_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(
            container.image,
            registry_image(
                TWOSAMPLEMR_ORIGINAL_IMAGE_REPOSITORY,
                TWOSAMPLEMR_ORIGINAL_IMAGE_DIGEST
            )
            .unwrap()
        );
        assert!(container.panel_bundles.is_empty());
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(container.outputs.len(), 2);
        assert_eq!(container.outputs[0].path, "harmonised.tsv");
        assert_eq!(container.outputs[1].path, "harmonise.log");
        let script = container.script.unwrap();
        assert!(script.contains("TwoSampleMR::read_exposure_data"));
        assert!(script.contains("TwoSampleMR::read_outcome_data"));
        assert!(script.contains("TwoSampleMR::harmonise_data"));
        assert!(script.contains("AUTONOMICS_INPUT1"));
        // The downstream wrapper reads `data$snp` (lowercase), while
        // harmonise_data emits `SNP`; the rewrite must cover both cases.
        assert!(script.contains("names(harm)[names(harm) == \"SNP\"] <- \"snp\""));
        assert!(script.contains("gsub(\"\\\\.\", \"_\""));
        assert!(script.contains("\"ieu-a-2\""));
        assert!(script.contains("\"Body mass index\""));
        assert!(script.contains("action = 2"));
        assert!(!script.contains("units.exposure <-"));
        assert!(!script.contains("units.outcome <-"));
    }

    #[test]
    fn stamps_units_labels_when_provided() {
        let mut value = spec();
        value.units_exposure = Some("SD".into());
        value.units_outcome = Some("log odds".into());
        let script = container_spec(&value).unwrap().script.unwrap();
        assert!(script.contains("exp$units.exposure <- \"SD\""));
        assert!(script.contains("out$units.outcome <- \"log odds\""));
    }

    #[test]
    fn rejects_invalid_harmonise_action() {
        let mut value = spec();
        value.harmonise_action = 0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.harmonise_action = 4;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn rejects_empty_labels() {
        for field in ["id_exposure", "exposure", "id_outcome", "outcome"] {
            let mut value = spec();
            match field {
                "id_exposure" => value.id_exposure = "   ".into(),
                "exposure" => value.exposure = String::new(),
                "id_outcome" => value.id_outcome = String::new(),
                "outcome" => value.outcome = String::new(),
                _ => unreachable!(),
            }
            assert!(
                validate(&value).is_err(),
                "expected validate() to reject empty `{field}`",
            );
        }
    }
}
