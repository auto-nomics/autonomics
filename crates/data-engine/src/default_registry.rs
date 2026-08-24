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
    registry.register_plugin(&nodes_io::Plugin::default());
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

    // ── Phase 2: regression, survival, coloc, epi, viz, sql bundles ────
    #[cfg(feature = "bundle-regression")]
    registry.register_plugin(&nodes_regression::Plugin);
    #[cfg(feature = "bundle-survival")]
    registry.register_plugin(&nodes_survival::Plugin);
    #[cfg(feature = "bundle-coloc")]
    registry.register_plugin(&nodes_coloc::Plugin);
    #[cfg(feature = "bundle-epi")]
    registry.register_plugin(&nodes_epi::Plugin);
    #[cfg(feature = "bundle-viz")]
    registry.register_plugin(&nodes_viz::Plugin);
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
