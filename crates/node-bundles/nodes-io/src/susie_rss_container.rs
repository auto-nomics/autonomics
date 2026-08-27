//! Containerized SuSiE-RSS node backed by the official susieR R package.
//!
//! This wrapper owns the image / signed-LD panel binding so callers provide
//! one tab-separated GWAS sumstats File (`snp`, `chrom`, `z`, optional `n`,
//! `a1`, `a2`) and receive the official fit as TSV/RDS plus a log. The R
//! script queries signed LD pairs from the mounted `mixer.g1000_eur` catalog
//! panel and calls `susieR::susie_rss()` directly; no Rust SuSiE port is used.

use std::collections::BTreeMap;
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

pub const SUSIE_RSS_CONTAINER_KIND: &str = "susie_rss_container";
pub const SUSIE_ORIGINAL_IMAGE: &str = "192.168.10.24:30500/atc/susie@sha256:8a72a443461add5c94c4907f9a1d6106b989850e93217a95587d9095542febe8";
pub const SUSIE_REF_PANEL: &str = "mixer.g1000_eur";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/susie_rss_container";
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_L: usize = 10;
const DEFAULT_ESTIMATE_PRIOR_METHOD: &str = "optim";
const DEFAULT_ESTIMATE_RESIDUAL_VARIANCE: bool = false;
const DEFAULT_ESTIMATE_PRIOR_VARIANCE: bool = true;
const DEFAULT_COVERAGE: f64 = 0.95;
const DEFAULT_MIN_ABS_CORR: f64 = 0.5;
const DEFAULT_SCALED_PRIOR_VARIANCE: f64 = 0.2;
const DEFAULT_Z_METHOD: &str = "wald";
const DEFAULT_R2_MIN: f64 = 0.0;
const DEFAULT_CHECK_NULL_THRESHOLD: f64 = 0.0;
const DEFAULT_MAX_ITER: usize = 100;
const FACTORY_DOC: &str = r#"Runs the official susieR 0.16.6 susie_rss(z, R, n, ...) implementation in an ephemeral OCI container.

Input contract:
- One File input, strictly tab-separated, with columns snp, chrom, z and optional n, a1, a2.
- Produce it in a DAG with sink_file using format="tsv"; comma-separated input is parsed as one column and misleadingly fails with missing required columns.
- All rows must use one chromosome. SNP must be a literal mixer BIM ID in GRCh37 chr:pos:allele1:allele2 form (for example, 21:36119111:G:T). rsIDs do not align. Supplying a1/a2 in either allele orientation is supported; otherwise they are inferred from the ID.
- Keep SNP keys unique upstream. SuSiE-RSS is a single-locus model, so split large regions and consider r2_min>0 to sparsify LD.

The node mounts the mixer.g1000_eur signed-LD catalog panel, aligns against its full per-chromosome BIM, and publishes immutable outputs under artifact_prefix/job-id: port 0 susie_rss.tsv (snp, pip, cs, alpha, mu, mu2, lbf), port 1 susie_rss.RDS (the official fit object), and port 2 susie_rss.log (variant count, iterations, convergence, and credible sets). In the TSV, cs=0 means no credible set and cs=k>=1 means membership in credible set k. The roughly 254-SNP .snps extraction lists in the panel are mixer leftovers and are not an alignment constraint.

Defaults: l=10, estimate_prior_method=optim, estimate_residual_variance=false, estimate_prior_variance=true, coverage=0.95, min_abs_corr=0.5, scaled_prior_variance=0.2, z_method=wald, r2_min=0, check_null_threshold=0, max_iter=100, timeout_secs=1800, artifact_prefix=/artifacts/susie_rss_container."#;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SusieRssContainerSpec {
    /// Maximum number of non-zero effects (SuSiE L parameter). Default 10.
    #[serde(default = "default_l")]
    pub l: usize,
    /// Prior-variance optimization method: "optim", "EM", or "simple".
    #[serde(default = "default_estimate_prior_method")]
    pub estimate_prior_method: String,
    /// Estimate residual variance each IBSS iteration.
    #[serde(default = "default_estimate_residual_variance")]
    pub estimate_residual_variance: bool,
    /// Estimate prior variance per effect.
    #[serde(default = "default_estimate_prior_variance")]
    pub estimate_prior_variance: bool,
    /// Coverage for credible sets. Default 0.95.
    #[serde(default = "default_coverage")]
    pub coverage: f64,
    /// Min |corr| purity threshold for credible sets. Default 0.5.
    #[serde(default = "default_min_abs_corr")]
    pub min_abs_corr: f64,
    /// Scaled prior variance (initial value when `estimate_prior_variance=true`).
    #[serde(default = "default_scaled_prior_variance")]
    pub scaled_prior_variance: f64,
    /// z-score method: "wald" or "score".
    #[serde(default = "default_z_method")]
    pub z_method: String,
    /// Minimum r² threshold for signed LD pairs. Default 0.0.
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
    /// Sample size override. If None, reads `n` from the input column.
    #[serde(default)]
    pub n: Option<f64>,
    /// Check-null threshold passed to susie_rss.
    #[serde(default = "default_check_null_threshold")]
    pub check_null_threshold: f64,
    /// Maximum IBSS iterations. Default 100.
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
    /// VFS prefix used to publish immutable SuSiE artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    /// Maximum container runtime in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_l() -> usize {
    DEFAULT_L
}

fn default_estimate_prior_method() -> String {
    DEFAULT_ESTIMATE_PRIOR_METHOD.into()
}

fn default_estimate_residual_variance() -> bool {
    DEFAULT_ESTIMATE_RESIDUAL_VARIANCE
}

fn default_estimate_prior_variance() -> bool {
    DEFAULT_ESTIMATE_PRIOR_VARIANCE
}

fn default_coverage() -> f64 {
    DEFAULT_COVERAGE
}

fn default_min_abs_corr() -> f64 {
    DEFAULT_MIN_ABS_CORR
}

fn default_scaled_prior_variance() -> f64 {
    DEFAULT_SCALED_PRIOR_VARIANCE
}

fn default_z_method() -> String {
    DEFAULT_Z_METHOD.into()
}

fn default_r2_min() -> f64 {
    DEFAULT_R2_MIN
}

fn default_check_null_threshold() -> f64 {
    DEFAULT_CHECK_NULL_THRESHOLD
}

fn default_max_iter() -> usize {
    DEFAULT_MAX_ITER
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct SusieRssContainerNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl SusieRssContainerNodeFactory {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct SusieRssContainerNode {
    inner: Box<dyn DagNode>,
}

impl Clone for SusieRssContainerNode {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for SusieRssContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        SUSIE_RSS_CONTAINER_KIND
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

pub fn validate(spec: &SusieRssContainerSpec) -> Result<(), String> {
    if spec.l == 0 {
        return Err("l must be greater than zero".into());
    }
    if !matches!(
        spec.estimate_prior_method.as_str(),
        "optim" | "EM" | "em" | "simple"
    ) {
        return Err("estimate_prior_method must be one of: optim, EM, simple".into());
    }
    if !matches!(spec.z_method.as_str(), "wald" | "score") {
        return Err("z_method must be one of: wald, score".into());
    }
    if !(0.0..=1.0).contains(&spec.coverage) {
        return Err("coverage must lie in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.min_abs_corr) {
        return Err("min_abs_corr must lie in (0, 1]".into());
    }
    if !(0.0..=1.0).contains(&spec.r2_min) {
        return Err("r2_min must lie in (0, 1]".into());
    }
    if spec.scaled_prior_variance <= 0.0 {
        return Err("scaled_prior_variance must be greater than zero".into());
    }
    if spec.max_iter == 0 {
        return Err("max_iter must be greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    Ok(())
}

pub fn container_spec(spec: &SusieRssContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let mut env = BTreeMap::from([
        ("SUSIE_L".to_string(), spec.l.to_string()),
        (
            "SUSIE_ESTIMATE_PRIOR_METHOD".to_string(),
            spec.estimate_prior_method.clone(),
        ),
        (
            "SUSIE_ESTIMATE_RESIDUAL_VARIANCE".to_string(),
            spec.estimate_residual_variance.to_string(),
        ),
        (
            "SUSIE_ESTIMATE_PRIOR_VARIANCE".to_string(),
            spec.estimate_prior_variance.to_string(),
        ),
        ("SUSIE_COVERAGE".to_string(), spec.coverage.to_string()),
        (
            "SUSIE_MIN_ABS_CORR".to_string(),
            spec.min_abs_corr.to_string(),
        ),
        (
            "SUSIE_SCALED_PRIOR_VARIANCE".to_string(),
            spec.scaled_prior_variance.to_string(),
        ),
        ("SUSIE_Z_METHOD".to_string(), spec.z_method.clone()),
        ("SUSIE_R2_MIN".to_string(), spec.r2_min.to_string()),
        (
            "SUSIE_CHECK_NULL_THRESHOLD".to_string(),
            spec.check_null_threshold.to_string(),
        ),
        ("SUSIE_MAX_ITER".to_string(), spec.max_iter.to_string()),
    ]);
    if let Some(n) = spec.n {
        env.insert("SUSIE_N".to_string(), n.to_string());
    }

    Ok(ContainerCommandSpec {
        image: SUSIE_ORIGINAL_IMAGE.into(),
        command: vec!["Rscript".into(), "/opt/susie/bin/run_susie_rss.R".into()],
        script: None,
        files: BTreeMap::new(),
        env,
        outputs: vec![
            ContainerCommandOutputSpec {
                path: "susie_rss.tsv".into(),
                format: Some("susie_rss".into()),
            },
            ContainerCommandOutputSpec {
                path: "susie_rss.RDS".into(),
                format: Some("r_rds".into()),
            },
            ContainerCommandOutputSpec {
                path: "susie_rss.log".into(),
                format: Some("susie_rss_log".into()),
            },
        ],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: vec![ContainerPanelBundleSpec {
            panel_id: SUSIE_REF_PANEL.into(),
            mount_path: "/panels/mixer_ref".into(),
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
}

fn panel_bindings() -> Vec<DataBundleBinding> {
    vec![DataBundleBinding::new("susie_ref", SUSIE_REF_PANEL)]
}

impl NodeFactory for SusieRssContainerNodeFactory {
    fn kind(&self) -> &'static str {
        SUSIE_RSS_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs official susieR::susie_rss against the mixer signed-LD panel."
    }

    fn doc(&self) -> &'static str {
        FACTORY_DOC
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SusieRssContainerSpec)
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
        let spec: SusieRssContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = panel_bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::ContainerRuntime> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(SusieRssContainerNode {
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: SusieRssContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SusieRssContainerSpec {
        SusieRssContainerSpec {
            l: DEFAULT_L,
            estimate_prior_method: DEFAULT_ESTIMATE_PRIOR_METHOD.into(),
            estimate_residual_variance: DEFAULT_ESTIMATE_RESIDUAL_VARIANCE,
            estimate_prior_variance: DEFAULT_ESTIMATE_PRIOR_VARIANCE,
            coverage: DEFAULT_COVERAGE,
            min_abs_corr: DEFAULT_MIN_ABS_CORR,
            scaled_prior_variance: DEFAULT_SCALED_PRIOR_VARIANCE,
            z_method: DEFAULT_Z_METHOD.into(),
            r2_min: DEFAULT_R2_MIN,
            n: None,
            check_null_threshold: DEFAULT_CHECK_NULL_THRESHOLD,
            max_iter: DEFAULT_MAX_ITER,
            artifact_prefix: DEFAULT_ARTIFACT_PREFIX.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    #[test]
    fn builds_official_susie_image_and_panel_contract() {
        let container = container_spec(&spec()).unwrap();
        assert_eq!(container.image, SUSIE_ORIGINAL_IMAGE);
        assert_eq!(
            container.command,
            vec!["Rscript", "/opt/susie/bin/run_susie_rss.R"]
        );
        assert_eq!(container.pull_policy, PullPolicy::Missing);
        assert_eq!(container.network, "isolated");
        assert_eq!(container.panel_bundles.len(), 1);
        assert_eq!(container.panel_bundles[0].panel_id, SUSIE_REF_PANEL);
        assert_eq!(container.panel_bundles[0].mount_path, "/panels/mixer_ref");
        assert_eq!(container.outputs.len(), 3);
        assert_eq!(container.env.get("SUSIE_L").map(String::as_str), Some("10"));
        assert_eq!(
            container
                .env
                .get("SUSIE_ESTIMATE_PRIOR_METHOD")
                .map(String::as_str),
            Some("optim")
        );
        assert_eq!(
            container.env.get("SUSIE_MAX_ITER").map(String::as_str),
            Some("100")
        );
    }

    #[test]
    fn rejects_invalid_parameters() {
        let mut s = spec();
        s.l = 0;
        assert!(validate(&s).is_err());
        s.l = DEFAULT_L;
        s.estimate_prior_method = "unknown".into();
        assert!(validate(&s).is_err());
        s.estimate_prior_method = DEFAULT_ESTIMATE_PRIOR_METHOD.into();
        s.coverage = 1.5;
        assert!(validate(&s).is_err());
        s.coverage = DEFAULT_COVERAGE;
        s.r2_min = -0.1;
        assert!(validate(&s).is_err());
        s.r2_min = DEFAULT_R2_MIN;
        s.artifact_prefix = "artifacts/susie".into();
        assert!(validate(&s).is_err());
        s.artifact_prefix = DEFAULT_ARTIFACT_PREFIX.into();
        s.timeout_secs = 0;
        assert!(validate(&s).is_err());
    }

    #[test]
    fn ports_match_three_outputs() {
        let ports = port_layout();
        assert_eq!(ports.input_ports().len(), 1);
        assert_eq!(ports.output_ports().len(), 3);
    }

    #[test]
    fn factory_doc_covers_operational_blind_spots() {
        for required in [
            "chr:pos:allele1:allele2",
            "rsIDs do not align",
            "format=\"tsv\"",
            "cs=k>=1",
            ".snps extraction lists",
            "timeout_secs=1800",
        ] {
            assert!(
                FACTORY_DOC.contains(required),
                "factory doc is missing `{required}`"
            );
        }
    }
}
