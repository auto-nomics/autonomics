use std::sync::Arc;

use container_runtime::{ContainerExecutionInfra, PodmanConfig};
use dag_core::registry::NodeCtx;

#[test]
fn plugin_registers_multiomic_concordance_after_container_sweep() {
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        None,
    );
    let mut registry = dag_core::registry::NodeRegistry::new(ctx);
    let container_execution = Arc::new(ContainerExecutionInfra::from_config(PodmanConfig {
        program: "podman".into(),
        workspace_root: "/tmp/autonomics-bulk-rnaseq-workspace".into(),
        panel_cache_root: "/tmp/autonomics-bulk-rnaseq-panels".into(),
    }));
    registry.register_plugin(&nodes_io::Plugin::new(container_execution));
    let kinds: Vec<String> = registry.list_nodes().into_iter().map(|n| n.kind).collect();
    assert!(
        kinds.iter().any(|k| k == "multiomic_concordance"),
        "multiomic_concordance should be registered; have: {kinds:?}"
    );
    // The container-backed bulk-rnaseq nodes completed the migration sweep:
    // `limma_voom` and `wgcna` (bulk-rnaseq family) and `hyprcoloc` ship as
    // manifest plugins — no `*_container` kind remains in this registry.
    // Their registration is exercised by plugin_wave_e2e and the
    // bulk_rnaseq/hyprcoloc golden tests in container-plugin.
    for kind in [
        "limma_voom",
        "wgcna",
        "hyprcoloc",
        "limma_voom_container",
        "wgcna_container",
        "hyprcoloc_container",
    ] {
        assert!(
            !kinds.iter().any(|k| k == kind),
            "{kind} must not come from the compile-time registry"
        );
    }
}
