//! Registration of all built-in node factories into a [`NodeRegistry`].
//!
//! All node implementations live in bundle crates under `crates/node-bundles/`.
//! This module registers them via Cargo features. Called once at engine startup
//! by [`crate::data_engine::DataEngine`].

use std::sync::Arc;

use datafusion::execution::runtime_env::RuntimeEnv;

use dag_core::{DataBundleCatalog, registry::NodeRegistry};

/// Build a [`NodeRegistry`] populated with every built-in node factory.
///
/// Node bundles are registered via Cargo features (default: all enabled).
/// To create a minimal engine, disable default features and select only
/// the bundles you need:
///
/// ```toml
/// data-engine = { default-features = false, features = ["bundle-mr", "bundle-io"] }
/// ```
pub fn build_default_registry(
    runtime_env: Arc<RuntimeEnv>,
    opendal: Option<Arc<vfs::OpendalFileStorage>>,
    data_bundles: Arc<DataBundleCatalog>,
) -> NodeRegistry {
    build_default_registry_with_container_execution(
        runtime_env,
        opendal,
        data_bundles,
        Arc::new(container_runtime::ContainerExecutionInfra::from_env()),
    )
}

/// Same as [`build_default_registry`], but with infrastructure explicitly owned
/// by the runtime host rather than implicitly created by the IO bundle.
pub fn build_default_registry_with_container_execution(
    runtime_env: Arc<RuntimeEnv>,
    opendal: Option<Arc<vfs::OpendalFileStorage>>,
    data_bundles: Arc<DataBundleCatalog>,
    container_execution: Arc<container_runtime::ContainerExecutionInfra>,
) -> NodeRegistry {
    let data_bundles = crate::data_bundles::catalog_with_builtins(&data_bundles);
    let mut registry = NodeRegistry::new(
        dag_core::registry::NodeCtx::new(runtime_env, opendal)
            .with_data_bundle_catalog(data_bundles),
    );

    // ── Phase 4: LDSC + genetics bundles ──────────────────────────────
    #[cfg(feature = "bundle-ldsc")]
    registry.register_plugin(&nodes_ldsc::Plugin);
    #[cfg(feature = "bundle-genetics")]
    registry.register_plugin(&nodes_genetics::Plugin);

    // ── MICE bundle ──────────────────────────────────────────────────
    #[cfg(feature = "bundle-mice")]
    registry.register_plugin(&nodes_mice::Plugin);

    // ── Writing bundle ────────────────────────────────────────────────
    #[cfg(feature = "bundle-writing")]
    registry.register_plugin(&nodes_writing::Plugin);

    // ── GRF bundle ────────────────────────────────────────────────────
    #[cfg(feature = "bundle-grf")]
    registry.register_plugin(&nodes_grf::Plugin);
    #[cfg(feature = "bundle-rd")]
    registry.register_plugin(&nodes_rd::Plugin);

    // ── Phase 3: IO, causal, lcmm, mr, survey bundles ──────────────────
    #[cfg(feature = "bundle-io")]
    registry.register_plugin(&nodes_io::Plugin::new(container_execution));
    #[cfg(feature = "bundle-opengwas")]
    registry.register_plugin(&nodes_opengwas::Plugin);
    #[cfg(feature = "bundle-causal")]
    registry.register_plugin(&nodes_causal::Plugin);
    #[cfg(feature = "bundle-lcmm")]
    registry.register_plugin(&nodes_lcmm::Plugin);
    #[cfg(feature = "bundle-mr")]
    registry.register_plugin(&nodes_mr::Plugin);
    #[cfg(feature = "bundle-survey")]
    registry.register_plugin(&nodes_survey::Plugin);

    // ── Phase 2: regression, survival, coloc, epi, sql bundles ────────
    #[cfg(feature = "bundle-regression")]
    registry.register_plugin(&nodes_regression::Plugin);
    #[cfg(feature = "bundle-survival")]
    registry.register_plugin(&nodes_survival::Plugin);
    #[cfg(feature = "bundle-coloc")]
    registry.register_plugin(&nodes_coloc::Plugin);
    #[cfg(feature = "bundle-epi")]
    registry.register_plugin(&nodes_epi::Plugin);
    #[cfg(feature = "bundle-sql")]
    registry.register_plugin(&nodes_sql::Plugin);

    // ── Phase 1: ml, hypothesize bundles ───────────────────────────────
    #[cfg(feature = "bundle-ml")]
    registry.register_plugin(&nodes_ml::Plugin);
    #[cfg(feature = "bundle-hypothesize")]
    registry.register_plugin(&nodes_hypothesize::Plugin);

    // ── Phase 1.5: DL bundle ────────────────────────────────────────────
    #[cfg(feature = "bundle-dl")]
    registry.register_plugin(&nodes_dl::Plugin);

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_container_command_factory_is_not_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: "/tmp/autonomics-registry-workspace".into(),
                    panel_cache_root: "/tmp/autonomics-registry-panels".into(),
                },
            ));
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(DataBundleCatalog::new()),
            container_execution,
        );

        assert!(
            !registry
                .list_nodes()
                .iter()
                .any(|node| node.kind == "container_command"),
            "container_command must not be exposed to agents"
        );
        assert!(registry.get_node_ports("container_command").is_err());
    }

    #[test]
    fn visualization_container_factory_is_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: "/tmp/autonomics-visualization-workspace".into(),
                    panel_cache_root: "/tmp/autonomics-visualization-panels".into(),
                },
            ));
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(DataBundleCatalog::new()),
            container_execution,
        );

        let ports = registry
            .get_node_ports("visualization_container")
            .expect("visualization_container is registered");
        assert!(
            registry.get_node_ports("visualization").is_err(),
            "the legacy host-R visualization node must not be registered"
        );
        assert_eq!(ports.input_ports().len(), 2);
        assert_eq!(ports.output_ports().len(), 1);
    }

    #[test]
    fn timesfm_container_factory_is_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: "/tmp/autonomics-timesfm-registry-workspace".into(),
                    panel_cache_root: "/tmp/autonomics-timesfm-registry-panels".into(),
                },
            ));
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(DataBundleCatalog::new()),
            container_execution,
        );

        let ports = registry
            .get_node_ports("timesfm_forecast_container")
            .expect("timesfm_forecast_container is registered");
        assert_eq!(ports.input_ports().len(), 1);
        assert_eq!(ports.output_ports().len(), 2);
    }

    #[test]
    fn string_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry =
            build_default_registry(runtime_env, None, Arc::new(DataBundleCatalog::new()));

        for kind in [
            "source_string_id_map",
            "source_string_network",
            "source_string_enrichment",
            "source_string_ppi_enrichment",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.output_ports().len(), 1);
        }
    }

    #[test]
    fn kegg_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: "/tmp/autonomics-kegg-registry-workspace".into(),
                    panel_cache_root: "/tmp/autonomics-kegg-registry-panels".into(),
                },
            ));
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(DataBundleCatalog::new()),
            container_execution,
        );

        let search = registry
            .get_node_ports("source_kegg_search")
            .expect("source_kegg_search is registered");
        assert_eq!(search.input_ports().len(), 0);
        assert_eq!(search.output_ports().len(), 1);

        let relations = registry
            .get_node_ports("source_kegg_relations")
            .expect("source_kegg_relations is registered");
        assert_eq!(relations.input_ports().len(), 1);
        assert_eq!(relations.output_ports().len(), 1);

        let pathways = registry
            .get_node_ports("source_kegg_gene_pathways")
            .expect("source_kegg_gene_pathways is registered");
        assert_eq!(pathways.input_ports().len(), 1);
        assert_eq!(pathways.output_ports().len(), 2);
    }
}
