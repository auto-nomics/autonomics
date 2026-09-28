use std::sync::Arc;

use container_runtime::{ContainerExecutionInfra, PodmanConfig};
use dag_core::registry::NodeCtx;

#[test]
fn plugin_registers_limma_voom_wgcna_and_multiomic_concordance() {
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
    for kind in [
        "limma_voom_container",
        "wgcna_container",
        // deseq2_de moved to the manifest plugin; it registers only when a
        // plugins root is configured (see plugin_wave_e2e).
        "multiomic_concordance",
    ] {
        assert!(
            kinds.iter().any(|k| k == kind),
            "{kind} should be registered; have: {kinds:?}"
        );
    }
}
