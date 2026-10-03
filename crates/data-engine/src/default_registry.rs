//! Registration of all built-in node factories into a [`NodeRegistry`].
//!
//! All node implementations live in bundle crates under `crates/node-bundles/`.
//! This module registers them via Cargo features. Called once at engine startup
//! by [`crate::data_engine::DataEngine`].

use std::sync::Arc;

use datafusion::execution::runtime_env::RuntimeEnv;

use dag_core::{BundleRegistry, registry::NodeRegistry};

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
    bundle_registry: Arc<BundleRegistry>,
) -> NodeRegistry {
    build_default_registry_with_container_execution(
        runtime_env,
        opendal,
        bundle_registry,
        Arc::new(container_runtime::ContainerExecutionInfra::from_env()),
        None,
    )
}

/// Same as [`build_default_registry`], but with infrastructure explicitly owned
/// by the runtime host rather than implicitly created by the IO bundle.
pub fn build_default_registry_with_container_execution(
    runtime_env: Arc<RuntimeEnv>,
    opendal: Option<Arc<vfs::OpendalFileStorage>>,
    bundle_registry: Arc<BundleRegistry>,
    container_execution: Arc<container_runtime::ContainerExecutionInfra>,
    plugins_root: Option<std::path::PathBuf>,
) -> NodeRegistry {
    let bundle_registry = crate::data_bundles::registry_with_builtins(&bundle_registry);
    let mut registry = NodeRegistry::new(
        dag_core::registry::NodeCtx::new(runtime_env, opendal)
            .with_bundle_registry(bundle_registry),
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

    // ── Regression discontinuity bundle ───────────────────────────────
    #[cfg(feature = "bundle-rd")]
    registry.register_plugin(&nodes_rd::Plugin);

    // ── Phase 3: IO, causal, lcmm, mr, survey bundles ──────────────────
    #[cfg(feature = "bundle-io")]
    registry.register_plugin(&nodes_io::Plugin::new());
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
    #[cfg(feature = "bundle-power")]
    registry.register_plugin(&nodes_power::Plugin);

    // ── Phase 1.5: DL bundle ────────────────────────────────────────────
    #[cfg(feature = "bundle-dl")]
    registry.register_plugin(&nodes_dl::Plugin);

    // ── Manifest plugin families (opt-in) ─────────────────────────────
    // A missing plugins root means "nothing installed" and is skipped;
    // an existing root with invalid manifests aborts startup, naming the
    // offending plugin. The root is admin-controlled host state: agents
    // have no path that reaches it.
    {
        let root = plugins_root.unwrap_or_else(container_plugin::loader::default_plugins_root);
        if root.is_dir() {
            let plugins = container_plugin::loader::load(
                &root,
                Arc::clone(&container_execution.runtime),
                Arc::clone(&container_execution.panel_cache),
            )
            .unwrap_or_else(|error| panic!("invalid plugin under `{}`: {error}", root.display()));
            for plugin in plugins {
                registry.register_plugin(&plugin);
            }
        }
    }

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
            Arc::new(BundleRegistry::new()),
            container_execution,
            None,
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
    fn visualization_wrapper_is_unregistered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: "/tmp/autonomics-visualization-workspace".into(),
                    panel_cache_root: "/tmp/autonomics-visualization-panels".into(),
                },
            ));
        // Pin the plugins root to a path that does not exist: the assertion
        // below is about a *plugin-free* registry build, and the default
        // root (`~/.autonomics/plugins`) exists on dev machines with
        // plugins installed — where the manifest `visualization` plugin
        // legitimately registers its kind.
        let plugins_parent = tempfile::tempdir().unwrap();
        let plugins_root = plugins_parent.path().join("absent");
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(BundleRegistry::new()),
            container_execution,
            Some(plugins_root),
        );

        // The wrapper moved to the manifest plugin: neither the wrapper
        // kind nor the plugin kind appears in a plugin-free default
        // registry build.
        assert!(registry.get_node_ports("visualization_container").is_err());
        assert!(
            registry.get_node_ports("visualization").is_err(),
            "the legacy host-R visualization node must not be registered"
        );
    }

    #[test]
    fn string_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

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
    fn enrichr_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

        for kind in [
            "source_enrichr_enrich",
            "source_enrichr_libraries",
            "source_enrichr_view_list",
            "source_enrichr_genemap",
            "source_enrichr_background_enrich",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.input_ports().len(), 0);
            assert_eq!(ports.output_ports().len(), 1);
        }
    }

    #[test]
    fn gwascatalog_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

        for kind in [
            "source_gwascatalog_search",
            "source_gwascatalog_studies",
            "source_gwascatalog_associations",
            "source_gwascatalog_study_associations",
            "source_gwascatalog_snps",
            "source_gwascatalog_efo_traits",
            "source_gwascatalog_unpublished_studies",
            "source_gwascatalog_summary_associations",
            "source_gwascatalog_download",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.input_ports().len(), 0);
            assert_eq!(ports.output_ports().len(), 1);
        }
    }

    /// The tool→node migration (dag-generalization survey B2): OpenTargets
    /// ranked associations, ChEMBL mechanism/indication tables, and the KEGG
    /// drug–drug interaction table are the node-side counterparts added for
    /// the previously tool-only capabilities.
    #[test]
    fn b2_gap_source_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

        for kind in [
            "source_opentargets_associated_diseases",
            "source_opentargets_associated_targets",
            "source_chembl_mechanisms",
            "source_chembl_indications",
            "source_kegg_ddi",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.output_ports().len(), 1);
        }
    }

    /// B5 (M3-② spike): the scalar and matrix OpenGWAS operations fold into
    /// the DataFrame channel — scalar as a single-row DataFrame, matrix as
    /// a long-format table. No new `PortType`s.
    #[test]
    fn b5_scalar_matrix_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

        for kind in [
            "source_opengwas_gwasinfo_count",
            "source_opengwas_ld_matrix",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.input_ports().len(), 0);
            assert_eq!(ports.output_ports().len(), 1);
        }
    }

    /// B6: the stat-side algorithm wrappers named in the survey §2.4
    /// (`competing_risk` is already covered by `cuminc` + `fine_gray`).
    #[test]
    fn b6_stat_factories_are_registered() {
        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let registry = build_default_registry(runtime_env, None, Arc::new(BundleRegistry::new()));

        for kind in [
            "epi_multistate",
            "epi_gbtm",
            "epi_lca",
            "epi_rf_shap",
            "epi_cfa",
        ] {
            let ports = registry
                .get_node_ports(kind)
                .unwrap_or_else(|error| panic!("{kind} must be registered: {error}"));
            assert_eq!(ports.input_ports().len(), 1);
            assert!(!ports.output_ports().is_empty());
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
            Arc::new(BundleRegistry::new()),
            container_execution,
            None,
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

    #[test]
    fn manifest_plugins_register_alongside_builtin_bundles() {
        // The root is passed explicitly: no process-global env mutation,
        // so parallel registry builds in other tests are unaffected.
        let plugins_root = tempfile::tempdir().unwrap();
        let ldsc_dir = plugins_root.path().join("ldsc");
        std::fs::create_dir_all(ldsc_dir.join("scripts")).unwrap();
        std::fs::write(
            ldsc_dir.join("scripts/h2.sh"),
            "set -eu\nldsc --h2 \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\" 2>&1\n",
        )
        .unwrap();
        std::fs::write(
            ldsc_dir.join(container_plugin::loader::MANIFEST_FILE),
            r#"
schema_version = 1
plugin_name = "ldsc"

[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"

[[nodes]]
kind = "ldsc_h2_plugin_test"
desc = "d"
doc = "doc"
timeout_secs = 3600

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [{ path = "out.log", format = "ldsc_log" }]

[nodes.command]
interpreter = "sh"
script_file = "scripts/h2.sh"
"#,
        )
        .unwrap();

        let runtime_env = datafusion::prelude::SessionContext::new().runtime_env();
        let container_execution =
            Arc::new(container_runtime::ContainerExecutionInfra::from_config(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: plugins_root.path().join("workspace"),
                    panel_cache_root: plugins_root.path().join("panels"),
                },
            ));
        let registry = build_default_registry_with_container_execution(
            runtime_env,
            None,
            Arc::new(dag_core::BundleRegistry::new()),
            container_execution,
            Some(plugins_root.path().to_path_buf()),
        );

        // The manifest kind is registered alongside every builtin bundle...
        assert!(
            registry
                .list_nodes()
                .iter()
                .any(|node| node.kind == "ldsc_h2_plugin_test"),
            "manifest plugin kind must appear in the registry"
        );
        // ...and the curation gate still hides the generic primitive.
        assert!(registry.get_node_ports("container_command").is_err());
    }
}
