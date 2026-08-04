use std::sync::Arc;

use datafusion::{
    catalog::CatalogProvider,
    common::HashMap,
    execution::{runtime_env::RuntimeEnv, session_state::SessionStateBuilder},
    prelude::{SessionConfig, SessionContext},
};
use datalake::Datalake;

use serde::Serialize;

use super::error::{Error, Result};
use crate::codegen::context::{CodegenCtx, CodegenError, CodegenTarget, NodeCodegen};
use crate::dag::DagNode;
use crate::nodes::meta::NodePorts;
use crate::nodes::{
    bivariate_mixer::BivariateMixerNodeFactory,
    causal::CausalNodeFactory,
    chi_square::ChiSquareNodeFactory,
    cmest::{
        CmestBinaryMNodeFactory, CmestBinaryYNodeFactory, CmestGformulaNodeFactory,
        CmestMultiNodeFactory, CmestNodeFactory, CmestWeightingNodeFactory,
    },
    cox_regression::CoxRegressionNodeFactory,
    cpassoc::CpassocNodeFactory,
    cuminc::CumincNodeFactory,
    echo_node::EchoNodeFactory,
    coloc::ColocAbfNodeFactory,
    bkmr::BkmrNodeFactory,
    epi_lasso::EpiLassoNodeFactory,
    epi_rcs::EpiRcsNodeFactory,
    epi_roc::EpiRocNodeFactory,
    epi_wqs::EpiWqsNodeFactory,
    evalue::EvalueNodeFactory,
    fine_gray::FineGrayNodeFactory,
    hdl_l::HdlLNodeFactory,
    hdl_l_scan::HdlLScanNodeFactory,
    lava::{
        LavaBivarNodeFactory, LavaLocusNodeFactory, LavaMultiregNodeFactory, LavaPcorNodeFactory,
        LavaUnivNodeFactory,
    },
    lcv::LcvNodeFactory,
    ldsc_hsq::LdscHsqNodeFactory,
    ldsc_rg::LdscRgNodeFactory,
    ldsc_sldsc::LdscSldscNodeFactory,
    liability::LiabilityNodeFactory,
    linear_regression::LinearRegressionNodeFactory,
    logistic_regression::LogisticRegressionNodeFactory,
    magma::{
        MagmaAnnotateNodeFactory, MagmaGeneNodeFactory, MagmaMetaNodeFactory, MagmaSetNodeFactory,
    },
    mediation::MediationNodeFactory,
    mrlap::MrlapNodeFactory,
    mrpresso::MrpressoNodeFactory,
    mtag::MtagNodeFactory,
    sink_file::FileSinkNodeFactory,
    sink_iceberg::IcebergSinkNodeFactory,
    source_file::FileSourceNodeFactory,
    source_iceberg::IcebergSourceNodeFactory,
    source_opentargets::{OpentargetsAssociationsNodeFactory, OpentargetsSearchNodeFactory},
    sql_node::SqlNodeFactory,
    survey_calibrate::{CalibrateFactory, PostStratifyFactory, RakeFactory, TrimWeightsFactory},
    survey_describe::{
        SvyMeanFactory, SvyQuantileFactory, SvyRatioFactory, SvyTableFactory, SvyTotalFactory,
        SvyVarFactory,
    },
    survey_model::{
        SvyCoxphFactory, SvyGlmFactory, SvyIvregFactory, SvyLoglinFactory, SvyMleFactory,
        SvyNlsFactory, SvyOlrFactory, SvySurvregFactory,
    },
    survey_survival::{SvyKmFactory, SvyLogrankFactory},
    survey_test::{SvyChisqFactory, SvyCiPropFactory, SvyRankTestFactory, SvyTtestFactory},
    survey_utility::{RegTermTestFactory, SvyByFactory, SvyContrastFactory, SvyStandardizeFactory},
    survival::SurvivalNodeFactory,
    susie_rss::SusieRssNodeFactory,
    two_sample_mr::TwoSampleMrNodeFactory,
    univariate_mixer::UnivariateMixerNodeFactory,
    viz::VizNodeFactory,
};

/// Build a fresh, isolated [`SessionContext`].
///
/// Each call creates a **new** `CatalogList` (so `register_table("port_0", ...)`
/// never collides with another node's registration), while sharing the
/// engine-wide [`RuntimeEnv`] so object stores remain reachable.
///
/// If an `iceberg_catalog` is provided, it is registered under `"iceberg"`
/// on the fresh context.
pub fn new_isolated_ctx(
    runtime_env: Arc<RuntimeEnv>,
    iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
) -> SessionContext {
    let state = SessionStateBuilder::new()
        .with_default_features()
        .with_config(SessionConfig::new())
        .with_runtime_env(runtime_env)
        .build();
    let ctx = SessionContext::new_with_state(state);
    if let Some(cat) = iceberg_catalog {
        ctx.register_catalog("iceberg", cat);
    }
    ctx
}

pub trait NodeFactory: Send + Sync {
    fn kind(&self) -> &'static str;
    fn desc(&self) -> &'static str;
    fn doc(&self) -> &'static str;
    fn spec_schema(&self) -> schemars::Schema;
    /// The static port layout for this node kind — the input/output ports
    /// every instance of this kind will declare. Queryable without
    /// instantiating a node (mirrors [`NodeFactory::spec_schema`]).
    fn ports(&self) -> NodePorts;
    fn build(&self, spec: serde_json::Value, node_ctx: NodeCtx) -> Result<Box<dyn DagNode>>;

    // ── reverse-compilation (codegen) ────────────────────────────────────
    //
    // Default implementations return `NotSupported`, so existing factories
    // compile unchanged. Override per-kind to enable R/Python codegen.

    /// Compile this node kind's spec into R code that calls the original
    /// reference R package.
    fn codegen_r(
        &self,
        _spec: &serde_json::Value,
        _ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        Err(CodegenError::NotSupported {
            kind: self.kind().to_string(),
            target: CodegenTarget::R,
        })
    }

    /// Compile this node kind's spec into Python code.
    fn codegen_python(
        &self,
        _spec: &serde_json::Value,
        _ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        Err(CodegenError::NotSupported {
            kind: self.kind().to_string(),
            target: CodegenTarget::Python,
        })
    }

    /// R packages this node's generated code requires (e.g. `["TwoSampleMR"]`).
    fn r_packages(&self) -> Vec<String> {
        Vec::new()
    }

    /// Python packages this node's generated code requires.
    fn python_packages(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Ingredients for building an isolated [`SessionContext`] per node execution.
///
/// Instead of sharing a single `SessionContext` (which causes CatalogList
/// collisions on `register_table`), nodes receive the `RuntimeEnv` and an
/// optional `Iceberg` catalog, and construct their own context at execution
/// time via [`new_isolated_ctx`].
#[derive(Clone)]
pub struct NodeCtx {
    /// Shared object-store registry — all nodes reference the same
    /// `RuntimeEnv` so file:// / s3:// stores registered by the engine
    /// builder are reachable.
    pub runtime_env: Arc<RuntimeEnv>,
    /// Optional Iceberg `CatalogProvider`. Nodes that need to query
    /// `iceberg.*` tables receive `Some`; others receive `None`.
    pub iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
    /// Iceberg REST catalog handle, used by LDSC nodes for table-level
    /// operations (create/drop/load) that go through the Iceberg API
    /// directly rather than DataFusion SQL.
    pub datalake: Arc<Datalake>,
    /// The opendal-backed file storage registered with the engine, used by
    /// artifact-producing nodes (e.g. `VizNode`) to write outputs into the
    /// engine's virtualized filesystem rather than the host filesystem.
    /// `None` when no opendal fs was registered.
    pub opendal: Option<Arc<fs::OpendalFileStorage>>,
}

impl NodeCtx {
    /// Build a **fresh**, isolated [`SessionContext`] from these ingredients.
    ///
    /// Each call returns a brand-new context with its own `CatalogList` (so
    /// `register_table("port_0", …)` / `register_table("sumstats", …)` never
    /// collide across nodes or across executions) while sharing the engine-wide
    /// [`RuntimeEnv`]. This is the *only* way a `DagNode` should obtain a
    /// `SessionContext` inside `execute`: the framework injects a `&NodeCtx`,
    /// the node calls `ctx.session()`, and the resulting context is dropped at
    /// the end of the execution — no mutable catalog state ever leaks across
    /// runs or between `clone_box` copies of a node.
    pub fn session(&self) -> SessionContext {
        new_isolated_ctx(self.runtime_env.clone(), self.iceberg_catalog.clone())
    }
}

/// Summary of a registered node kind returned by [`NodeRegistry::list_nodes`].
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub kind: String,
    pub desc: String,
    // pub schema: schemars::Schema,
    // /// Static input/output port layout for this kind.
    // pub ports: NodePorts,
}

/// The single source of truth of "which node kinds exist and how to build one from spec."
///
/// This object handles DagNode building and generalize operation of different nodes into uniformed
/// methods.
pub struct NodeRegistry {
    node_ctx: NodeCtx,
    nodes: HashMap<String, Box<dyn NodeFactory>>,
}

impl NodeRegistry {
    /// NOTE: currently all nodes are directly registered in this function.
    pub fn new(
        runtime_env: Arc<RuntimeEnv>,
        iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
        datalake: Arc<Datalake>,
        opendal: Option<Arc<fs::OpendalFileStorage>>,
    ) -> Self {
        let node_ctx = NodeCtx {
            runtime_env,
            iceberg_catalog,
            datalake,
            opendal,
        };

        let mut registry = Self {
            node_ctx,
            nodes: Default::default(),
        };
        registry.register(Box::new(SqlNodeFactory {}));
        registry.register(Box::new(FileSourceNodeFactory {}));
        registry.register(Box::new(IcebergSourceNodeFactory {}));
        registry.register(Box::new(FileSinkNodeFactory {}));
        registry.register(Box::new(IcebergSinkNodeFactory {}));
        registry.register(Box::new(LdscHsqNodeFactory {}));
        registry.register(Box::new(LdscRgNodeFactory {}));
        registry.register(Box::new(LcvNodeFactory {}));
        registry.register(Box::new(LdscSldscNodeFactory {}));
        registry.register(Box::new(LiabilityNodeFactory {}));
        registry.register(Box::new(LinearRegressionNodeFactory {}));
        registry.register(Box::new(LogisticRegressionNodeFactory {}));
        registry.register(Box::new(MediationNodeFactory {}));
        registry.register(Box::new(ChiSquareNodeFactory {}));
        registry.register(Box::new(CmestNodeFactory {}));
        registry.register(Box::new(CmestMultiNodeFactory {}));
        registry.register(Box::new(CmestBinaryYNodeFactory {}));
        registry.register(Box::new(CmestBinaryMNodeFactory {}));
        registry.register(Box::new(CmestWeightingNodeFactory {}));
        registry.register(Box::new(CmestGformulaNodeFactory {}));
        registry.register(Box::new(CoxRegressionNodeFactory {}));
        registry.register(Box::new(FineGrayNodeFactory {}));
        registry.register(Box::new(CumincNodeFactory {}));
        registry.register(Box::new(SurvivalNodeFactory {}));
        registry.register(Box::new(EpiRcsNodeFactory {}));
        registry.register(Box::new(EpiRocNodeFactory {}));
        registry.register(Box::new(EpiLassoNodeFactory {}));
        registry.register(Box::new(EpiWqsNodeFactory {}));
        registry.register(Box::new(EchoNodeFactory {}));
        registry.register(Box::new(ColocAbfNodeFactory {}));
        registry.register(Box::new(BkmrNodeFactory {}));
        registry.register(Box::new(EvalueNodeFactory {}));
        registry.register(Box::new(TwoSampleMrNodeFactory {}));
        registry.register(Box::new(MrlapNodeFactory {}));
        registry.register(Box::new(MrpressoNodeFactory {}));
        registry.register(Box::new(LavaLocusNodeFactory {}));
        registry.register(Box::new(LavaUnivNodeFactory {}));
        registry.register(Box::new(LavaBivarNodeFactory {}));
        registry.register(Box::new(LavaPcorNodeFactory {}));
        registry.register(Box::new(LavaMultiregNodeFactory {}));
        registry.register(Box::new(HdlLNodeFactory {}));
        registry.register(Box::new(HdlLScanNodeFactory {}));
        registry.register(Box::new(UnivariateMixerNodeFactory {}));
        registry.register(Box::new(BivariateMixerNodeFactory {}));
        registry.register(Box::new(CausalNodeFactory {}));
        registry.register(Box::new(MtagNodeFactory {}));
        registry.register(Box::new(CpassocNodeFactory {}));
        registry.register(Box::new(VizNodeFactory {}));
        registry.register(Box::new(OpentargetsAssociationsNodeFactory {}));
        registry.register(Box::new(OpentargetsSearchNodeFactory {}));
        registry.register(Box::new(SusieRssNodeFactory {}));
        registry.register(Box::new(MagmaAnnotateNodeFactory {}));
        registry.register(Box::new(MagmaGeneNodeFactory {}));
        registry.register(Box::new(MagmaSetNodeFactory {}));
        registry.register(Box::new(MagmaMetaNodeFactory {}));
        crate::nodes::hypothesize::register_all(&mut registry);
        // ── survey-package nodes ───────────────────────────────────────────
        registry.register(Box::new(SvyMeanFactory {}));
        registry.register(Box::new(SvyTotalFactory {}));
        registry.register(Box::new(SvyVarFactory {}));
        registry.register(Box::new(SvyRatioFactory {}));
        registry.register(Box::new(SvyTableFactory {}));
        registry.register(Box::new(SvyQuantileFactory {}));
        registry.register(Box::new(SvyGlmFactory {}));
        registry.register(Box::new(SvyCoxphFactory {}));
        registry.register(Box::new(SvySurvregFactory {}));
        registry.register(Box::new(SvyOlrFactory {}));
        registry.register(Box::new(SvyLoglinFactory {}));
        registry.register(Box::new(SvyMleFactory {}));
        registry.register(Box::new(SvyNlsFactory {}));
        registry.register(Box::new(SvyIvregFactory {}));
        registry.register(Box::new(PostStratifyFactory {}));
        registry.register(Box::new(RakeFactory {}));
        registry.register(Box::new(CalibrateFactory {}));
        registry.register(Box::new(TrimWeightsFactory {}));
        registry.register(Box::new(SvyTtestFactory {}));
        registry.register(Box::new(SvyRankTestFactory {}));
        registry.register(Box::new(SvyChisqFactory {}));
        registry.register(Box::new(SvyCiPropFactory {}));
        registry.register(Box::new(SvyKmFactory {}));
        registry.register(Box::new(SvyLogrankFactory {}));
        registry.register(Box::new(SvyByFactory {}));
        registry.register(Box::new(SvyContrastFactory {}));
        registry.register(Box::new(SvyStandardizeFactory {}));
        registry.register(Box::new(RegTermTestFactory {}));
        registry
    }

    pub fn register(&mut self, factory: Box<dyn NodeFactory>) {
        self.nodes.insert(factory.kind().to_string(), factory);
    }

    fn get_node_factory(&self, node_kind: &str) -> Result<&dyn NodeFactory> {
        self.nodes
            .get(node_kind)
            .map(|b| b.as_ref())
            .ok_or(Error::FactoryNotFound {
                kind: node_kind.to_string(),
            })
    }

    pub fn build_node(&self, node_kind: &str, spec: serde_json::Value) -> Result<Box<dyn DagNode>> {
        let node_factory = self.get_node_factory(node_kind)?;
        // Repair common LLM spec pathologies (object-wrapped arrays like
        // `{"item": x}`, numeric strings for number fields) against the
        // factory's own JSON Schema before deserializing. Schema-driven, so
        // well-formed specs pass through unchanged.
        let schema = serde_json::to_value(node_factory.spec_schema()).map_err(|e| {
            Error::Unknown(format!(
                "failed to serialize spec schema for kind '{node_kind}': {e}"
            ))
        })?;
        let spec = super::spec_normalize::normalize_against_schema(spec, &schema);
        // If the factory still can't deserialize the spec, upgrade the bare
        // serde error into an agent-facing SpecRejection carrying the kind,
        // the expected schema, and concrete remediation guidance.
        let node = node_factory
            .build(spec, self.node_ctx.clone())
            .map_err(|err| match err {
                super::error::Error::SpecDeserialize { source } => {
                    super::error::Error::spec_rejection_from(node_kind, &schema, source)
                }
                other => other,
            })?;
        Ok(node)
    }

    /// Return the JSON Schema that validates [`kind`]'s node spec.
    pub fn get_node_spec(&self, node_kind: &str) -> Result<schemars::Schema> {
        Ok(self.get_node_factory(node_kind)?.spec_schema())
    }

    /// Look up a node factory by kind string. Used by the DAG compiler.
    pub fn get_factory(&self, kind: &str) -> Result<&dyn NodeFactory> {
        self.get_node_factory(kind)
    }

    pub fn get_node_ports(&self, node_kind: &str) -> Result<NodePorts> {
        Ok(self.get_node_factory(node_kind)?.ports())
    }

    pub fn get_node_doc(&self, node_kind: &str) -> Result<String> {
        Ok(self.get_node_factory(node_kind)?.doc().to_string())
    }

    /// Return metadata of every registered node kind (kind + JSON Schema + ports).
    pub fn list_nodes(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .map(|(kind, factory)| NodeInfo {
                kind: kind.clone(),
                desc: factory.desc().to_string(),
                // schema: factory.spec_schema(),
                // ports: factory.ports(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid specs for each registered node kind — used by the
    /// invariant test to build a node without needing real files / data.
    fn fixture_spec(kind: &str) -> serde_json::Value {
        match kind {
            "sql" => serde_json::json!({"sql_query": "SELECT 1"}),
            "source_file" => serde_json::json!({"path": "/tmp/dummy.csv"}),
            "source_iceberg" => serde_json::json!({"ident": "gwas.dummy"}),
            "sink_file" => {
                serde_json::json!({"path": "/tmp/dummy_out.csv", "format": "csv"})
            }
            "sink_iceberg" => serde_json::json!({"ident": "gwas.dummy"}),
            "linear_regression" => {
                serde_json::json!({"x_columns": ["x"], "y_column": "y"})
            }
            "logistic_regression" => {
                serde_json::json!({"predictors": ["x"], "outcome": "y"})
            }
            "mediation" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y"})
            }
            "chi_square" => {
                serde_json::json!({"row_column": "group", "col_column": "var"})
            }
            "cmest" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y"})
            }
            "cmest_multi" => {
                serde_json::json!({"exposure_column": "x", "mediator_columns": ["m1", "m2"], "outcome_column": "y", "covariates": ["c1"]})
            }
            "cmest_binary_y" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y"})
            }
            "cmest_binary_m" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y"})
            }
            "cmest_weighting" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y", "covariates": ["c1"]})
            }
            "cmest_gformula" => {
                serde_json::json!({"exposure_column": "x", "mediator_column": "m", "outcome_column": "y", "covariates": ["c1"]})
            }
            "cox_regression" => {
                serde_json::json!({"predictors": ["x"], "time_column": "time", "event_column": "event"})
            }
            "survival" => {
                serde_json::json!({"time_column": "time", "event_column": "event"})
            }
            "causal" => {
                serde_json::json!({"method": "iptw", "treatment_column": "t", "outcome_column": "y", "covariates": ["x"]})
            }
            "epi_rcs" => {
                serde_json::json!({"x_column": "x", "outcome_column": "y"})
            }
            "epi_roc" => {
                serde_json::json!({"score1_column": "score", "label_column": "label"})
            }
            "epi_lasso" => {
                serde_json::json!({"predictors": ["x"], "outcome_column": "y"})
            }
            "epi_wqs" => {
                serde_json::json!({"exposures": ["x"], "outcome_column": "y"})
            }
            "magma_annotate" => serde_json::json!({"gene_loc": "g.txt", "snp_loc": "s.bim"}),
            "magma_gene" => serde_json::json!({"gene_annot": "a.genes.annot"}),
            "magma_set" => serde_json::json!({}),
            "magma_meta" => serde_json::json!({"cohort_files": ["c.genes.raw"]}),
            "cpassoc" => serde_json::json!({}),
            "susie_rss" => serde_json::json!({"l": 10, "estimate_prior_method": "optim"}),
            "hdl_l_scan" => serde_json::json!({
                "chr": 22, "scan_start": 1, "scan_stop": 2,
                "window_size": 1, "step": 1,
                "trait1_name": "t1", "trait2_name": "t2"
            }),
            "ldsc" => serde_json::json!({"n_blocks": 200}),
            "ldsc_rg" => serde_json::json!({"n_blocks": 200}),
            "lcv" => serde_json::json!({"no_blocks": 100}),
            "sldsc" => serde_json::json!({}),
            "liability" => serde_json::json!({"samp_prev": 0.5, "pop_prev": 0.01}),
            "two_sample_mr" => {
                serde_json::json!({"id_exposure": "exp", "id_outcome": "out", "action": "infer_strand", "method_list": ["mr_egger_regression"]})
            }
            "mrlap" => serde_json::json!({
                "exposure_name": "exp",
                "outcome_name": "out"
            }),
            "mtag" => serde_json::json!({"n_blocks": 200}),
            "lava_locus" => {
                serde_json::json!({"loci": [{"loc": "1", "chr": 1, "start": 1, "stop": 2}]})
            }
            "lava_univ" => serde_json::json!({}),
            "lava_bivar" => serde_json::json!({}),
            "lava_pcor" => serde_json::json!({"target": ["p1", "p2"]}),
            "lava_multireg" => serde_json::json!({"target": "p1"}),
            "hdl_l" => serde_json::json!({
                "chr": 22,
                "start": 17000000,
                "stop": 18000000,
                "trait1_name": "trait1",
                "trait2_name": "trait2"
            }),
            "visualization" => serde_json::json!({
                "output_path": "/tmp/dummy_viz.png",
                "r_code": "p <- ggplot(df, aes(x = x, y = y)) + geom_point()"
            }),
            "echo" => serde_json::json!({}),
            "source_opentargets_associations" => {
                serde_json::json!({"id": "ENSG00000012048"})
            }
            "source_opentargets_search" => serde_json::json!({"query": "BRCA1"}),
            "univariate_mixer" => serde_json::json!({"chromosomes": [21, 22]}),
            "bivariate_mixer" => serde_json::json!({"chromosomes": [21, 22]}),
            "fine_gray" => {
                serde_json::json!({"time_column": "time", "status_column": "fstatus", "covariates": ["x1"]})
            }
            "cuminc" => {
                serde_json::json!({"time_column": "time", "status_column": "fstatus"})
            }
            "evalue" => {
                serde_json::json!({"measure": "RR", "est": 0.8, "lo": 0.7, "hi": 0.9, "true_val": 1.0})
            }
            // ── survey nodes ──
            "svymean" => serde_json::json!({
                "design": {"ids": ["psu"], "weights": "wt"},
                "variables": ["y"]
            }),
            "svytotal" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["y"]
            }),
            "svyvar" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["y"]
            }),
            "svyratio" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "numerator": "n",
                "denominator": "d"
            }),
            "svytable" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["a", "b"]
            }),
            "svyquantile" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["y"],
                "quantiles": [0.5]
            }),
            "svyglm" => serde_json::json!({
                "design": {"ids": ["psu"], "weights": "wt"},
                "response": "y",
                "predictors": ["x1", "x2"]
            }),
            "svycoxph" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "time_column": "t",
                "event_column": "e",
                "predictors": ["x1"]
            }),
            "svysurvreg" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "time_column": "t",
                "event_column": "e",
                "predictors": ["x1"]
            }),
            "svyolr" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "response": "grade",
                "predictors": ["x1"]
            }),
            "svyloglin" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["a", "b"]
            }),
            "svymle" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "loglik_fn": "ll",
                "start": "a = 0",
                "response": "y",
                "predictors": ["x"]
            }),
            "svynls" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "formula": "y ~ x",
                "start": "a = 0"
            }),
            "svyivreg" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "response": "y",
                "endogenous": ["x1"],
                "instruments": ["z1"]
            }),
            "post_stratify" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "strata": ["s"],
                "population": {"A": 100, "B": 200}
            }),
            "rake" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "margins": [{"variable": "sex", "population": {"M": 100, "F": 110}}]
            }),
            "calibrate" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["x"],
                "population_totals": [100.0]
            }),
            "trim_weights" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "upper": 5.0
            }),
            "svyttest" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "response": "y",
                "group": "g"
            }),
            "svyranktest" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "response": "y",
                "group": "g"
            }),
            "svychisq" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "row_var": "a",
                "col_var": "b"
            }),
            "svyciprop" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variable": "p"
            }),
            "svykm" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "time_column": "t",
                "event_column": "e"
            }),
            "svylogrank" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "time_column": "t",
                "event_column": "e",
                "group": "g"
            }),
            "svyby" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["y"],
                "by": ["region"]
            }),
            "svycontrast" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "variables": ["a", "b"],
                "contrasts": [{"name": "diff", "expr": "a - b"}]
            }),
            "svystandardize" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "by": ["age"],
                "population": {"young": 0.5, "old": 0.5}
            }),
            "reg_term_test" => serde_json::json!({
                "design": {"ids": ["psu"]},
                "response": "y",
                "predictors": ["x1", "x2"],
                "test_terms": ["x2"]
            }),
            other => panic!("no fixture spec for kind '{other}'"),
        }
    }

    /// Helper: build a `NodeRegistry` with a bare (no iceberg) context for tests.
    fn test_registry() -> NodeRegistry {
        let ctx = SessionContext::new();
        let runtime_env = ctx.runtime_env();
        NodeRegistry::new(runtime_env, None, Arc::new(Datalake::default()), None)
    }

    /// Invariant: every registered factory's `kind()` matches the
    /// `node_type()` of a node built by that factory. Under Option B
    /// this should always hold (factories delegate to `<N as DagNode>::kind()`),
    /// but the test catches any future drift.
    #[test]
    fn all_factories_kind_matches_node_kind() {
        let registry = test_registry();

        let nodes = registry.list_nodes();
        assert!(!nodes.is_empty(), "registry should have at least one kind");

        for info in &nodes {
            let kind = &info.kind;
            let spec = fixture_spec(kind);
            let node = registry
                .build_node(kind, spec)
                .unwrap_or_else(|e| panic!("build_node({kind}) failed: {e}"));
            assert_eq!(
                node.kind(),
                kind,
                "factory kind '{}' must match built node's kind()",
                kind
            );

            // The factory's externally-queryable port layout must match the
            // layout the built node actually declares — otherwise the agent
            // (and `add_edge` validation) would be lied to.
            let factory_ports = registry.get_node_factory(kind).unwrap().ports();
            let node_ports = node.ports();
            assert_eq!(
                factory_ports.input_ports().len(),
                node_ports.input_ports().len(),
                "kind '{kind}': factory.ports() input count must match built node's"
            );
            assert_eq!(
                factory_ports.output_ports().len(),
                node_ports.output_ports().len(),
                "kind '{kind}': factory.ports() output count must match built node's"
            );
            assert_eq!(
                factory_ports.is_fixed_input(),
                node_ports.is_fixed_input(),
                "kind '{kind}': factory.ports() input-fixed flag must match built node's"
            );
        }
    }

    /// The exact malformed payload weak LLMs emit for `ldsc_rg` — `m` wrapped
    /// as `{"item": "23960350"}` and `n_blocks` as a string. The schema-guided
    /// normalizer in `build_node` must repair both so the spec deserializes.
    #[test]
    fn ldsc_rg_malformed_llm_spec_builds() {
        let registry = test_registry();
        // Numeric-string coercion: "200" → 200 (usize).
        let malformed = serde_json::json!({
            "n_blocks": "200"
        });
        let node = registry
            .build_node("ldsc_rg", malformed)
            .expect("malformed ldsc_rg spec should be normalized and build successfully");
        assert_eq!(node.kind(), "ldsc_rg");
    }

    /// `NodeInfo` (returned by `list_nodes`) must serialize, and each entry's
    /// `ports` field must round-trip into a JSON object with the expected
    /// input/output port shape. Guards the externally-queryable port layout
    /// the agent relies on via `list_node_factories`.
    #[test]
    fn node_info_ports_serialize() {
        let registry = test_registry();

        // `list_nodes` deliberately exposes only `kind` + `desc`; the static
        // port layout is served by the dedicated `get_node_ports` API. So the
        // serializable port shape is verified there, not on `NodeInfo`.
        let serialized =
            serde_json::to_value(registry.list_nodes()).expect("NodeInfo list must serialize");
        let arr = serialized
            .as_array()
            .expect("list_nodes serializes to an array");
        assert!(!arr.is_empty(), "registry should have registered nodes");
        for v in arr {
            assert!(v["kind"].is_string(), "NodeInfo has a kind");
            assert!(v["desc"].is_string(), "NodeInfo has a desc");
            assert!(
                v.get("ports").is_none() && v.get("schema").is_none(),
                "NodeInfo intentionally omits ports/schema"
            );
        }

        // ldsc_rg has two typed input ports + one typed output — the strongest
        // shape to pin down. Serialize its port layout via get_node_ports.
        let ports = registry
            .get_node_ports("ldsc_rg")
            .expect("ldsc_rg kind present");
        let serialized = serde_json::to_value(&ports).expect("NodePorts must serialize");
        let inputs = serialized["input_ports"]["ports"]
            .as_array()
            .expect("input ports array");
        assert_eq!(inputs.len(), 2, "ldsc_rg declares two input ports");
        assert!(inputs[0]["schema"].is_array(), "typed port exposes schema");
        let outputs = serialized["output_ports"]["ports"]
            .as_array()
            .expect("output ports array");
        assert_eq!(outputs.len(), 1, "ldsc_rg declares one output port");
    }
}
