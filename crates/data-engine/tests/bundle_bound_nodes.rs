//! Registry end-to-end checks for every node kind with runtime bundle bindings.

use std::sync::Arc;

use dag_core::{BundleRegistry, DataBundle};
use data_engine::data_engine::DataEngine;
use datafusion::prelude::SessionContext;

fn bundle(id: &str, vpath: &str) -> DataBundle {
    DataBundle::new(id, id, vpath)
}

fn catalog() -> BundleRegistry {
    BundleRegistry::from_bundles([
        bundle(
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_1000G_EUR,
            "/bundles/ldsc/1000g.parquet",
        ),
        bundle(
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_1000G_EUR_M,
            "/bundles/ldsc/1000g_m.parquet",
        ),
        bundle(
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_BASELINELD_V2_2_EUR,
            "/bundles/ldsc/baseline.parquet",
        ),
        bundle(
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_BASELINELD_V2_2_EUR_M,
            "/bundles/ldsc/baseline_m.parquet",
        ),
        bundle(
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_UKBB_EUR,
            "/bundles/ldsc/ukbb.parquet",
        ),
        bundle("ldmatrix-1000g-eur", "/bundles/ldmatrix/chr{N}"),
        bundle(
            nodes_genetics::magma_kegg::GENE_LOC_BUNDLE,
            "/bundles/magma/gene_loc.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_GENES_BUNDLE,
            "/bundles/kegg/genes.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_PATHWAY_KO_BUNDLE,
            "/bundles/kegg/pathway_ko.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_PATHWAYS_BUNDLE,
            "/bundles/kegg/pathways.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_GENOME_PATHWAYS_BUNDLE,
            "/bundles/kegg/genome_pathways.parquet",
        ),
    ])
    .unwrap()
}

fn registry() -> data_engine::node_registry::NodeRegistry {
    let ctx = SessionContext::new();
    // Hermetic: an absent plugins root keeps the default build free of
    // host-installed manifest plugins (~/.autonomics/plugins), which is
    // exactly the plugin-free registry these tests pin. Without it the
    // suite depends on what the developer machine has installed.
    let absent_plugins = std::env::temp_dir().join(format!(
        "autonomics-test-plugins-absent-{}",
        std::process::id()
    ));
    data_engine::default_registry::build_default_registry_with_container_execution(
        ctx.runtime_env(),
        None,
        Arc::new(catalog()),
        Arc::new(container_runtime::ContainerExecutionInfra::from_env()),
        Some(absent_plugins),
    )
}

#[test]
fn all_bundle_bound_node_kinds_build_from_runtime_catalog() {
    // Tool-container factories resolve immutable images against the fixed
    // GHCR namespace, so builds are deterministic without environment setup.
    let cases = [
        (
            "file_reference",
            serde_json::json!({
                "path": "/inputs/example.sumstats.gz",
                "format": "sumstats_gz"
            }),
        ),
        ("sldsc", serde_json::json!({})),
        (
            "lcv",
            serde_json::json!({
                "no_blocks": 5,
                "ldsc_intercept": true,
                "crosstrait_intercept": true,
                "sig_threshold": null,
                "intercept12": 0.0
            }),
        ),
        (
            "mrlap",
            serde_json::json!({"exposure_name": "exposure", "outcome_name": "outcome"}),
        ),
        (
            "magma_kegg_align",
            serde_json::json!({"min_set_size": 1, "max_set_size": 10}),
        ),
    ];
    let registry = registry();

    for (kind, spec) in cases {
        registry
            .build_node(kind, spec)
            .unwrap_or_else(|error| panic!("build `{kind}` through bundle registry: {error}"));
    }
}

#[test]
fn missing_catalog_bundle_rejects_node_build() {
    let ctx = SessionContext::new();
    let registry = data_engine::default_registry::build_default_registry(
        ctx.runtime_env(),
        None,
        Arc::new(BundleRegistry::new()),
    );

    let error = match registry.build_node(
        "bundle_source",
        serde_json::json!({"bundle_id": "missing.test", "format": "txt"}),
    ) {
        Ok(_) => panic!("missing custom bundle should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("missing.test"));
}

#[test]
fn global_builtin_bundles_build_without_a_runtime_catalog() {
    let ctx = SessionContext::new();
    let registry = data_engine::default_registry::build_default_registry(
        ctx.runtime_env(),
        None,
        Arc::new(BundleRegistry::new()),
    );

    registry
        .build_node("sldsc", serde_json::json!({}))
        .unwrap_or_else(|error| panic!("build `sldsc` from built-in catalog: {error}"));
    registry
        .build_node(
            "bundle_source",
            serde_json::json!({
                "bundle_id": "als_cns.cell_markers",
                "format": "tsv"
            }),
        )
        .unwrap_or_else(|error| panic!("build marker bundle source: {error}"));
}

#[test]
fn default_data_engine_uses_builtin_bundle_catalog() {
    let mut engine = DataEngine::builder().build();

    engine
        .add_node_from_registry("sldsc", "sldsc", serde_json::json!({}))
        .unwrap();
}

#[test]
fn native_hdl_l_nodes_are_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    assert!(!kinds.iter().any(|node| node.kind == "hdl_l"));
    // hdl_l_container / hdl_l_scan moved to the manifest plugin
    // (/mnt/projects/node-plugins/hdl); they no longer register in a
    // plugin-free default build.
}

#[test]
fn native_gsem_nodes_are_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native Rust GenomicSEM port (gsem_munge / gsem_ldsc /
    // gsem_usermodel / gsem_commonfactor / gsem_rgmodel) was removed from
    // nodes-genetics; no gsem node registers in a plugin-free default build.
    assert!(!kinds.iter().any(|node| node.kind.starts_with("gsem_")));
}

#[test]
fn native_magma_analysis_nodes_are_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native Rust MAGMA re-implementation (magma_gene / magma_set /
    // magma_meta) was removed from nodes-genetics; annotation already runs
    // through the `magma_annotate` manifest plugin and the official-binary
    // gene/set/meta wrap is tracked in docs/container-node-migration-backlog.md.
    // `magma_kegg_align` is pure KEGG-side data prep and stays registered.
    for kind in ["magma_gene", "magma_set", "magma_meta"] {
        assert!(!kinds.iter().any(|node| node.kind == kind), "{kind} still registered");
    }
    assert!(kinds.iter().any(|node| node.kind == "magma_kegg_align"));
}

#[test]
fn native_two_sample_mr_node_is_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native Rust TwoSampleMR port (`two_sample_mr`) and its
    // `bio_crates/mr` estimator library were removed; TwoSampleMR runs
    // through the `twosamplemr` manifest plugin. MRlap keeps its own
    // self-contained estimator in bio_crates/mrlap.
    assert!(!kinds.iter().any(|node| node.kind == "two_sample_mr"));
}

#[test]
fn native_bkmr_node_is_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native Rust BKMR port (`bkmr` node + `bio_crates/bkmr`) was
    // removed from nodes-coloc; no replacement runtime exists yet.
    assert!(!kinds.iter().any(|node| node.kind == "bkmr"));
}

#[test]
fn native_lava_nodes_are_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native Rust LAVA port (bio_crates/lava and the lava_locus /
    // lava_univ / lava_bivar / lava_pcor / lava_multireg node kinds) was
    // removed; LAVA runs through the `lava` / `lava_scan` manifest plugin.
    for kind in [
        "lava_locus",
        "lava_univ",
        "lava_bivar",
        "lava_pcor",
        "lava_multireg",
    ] {
        assert!(!kinds.iter().any(|node| node.kind == kind), "{kind} still registered");
    }
}

#[test]
fn native_coloc_abf_registration_is_removed() {
    let registry = registry();
    let kinds = registry.list_nodes();

    // The native coloc.abf reference implementation (bio_crates/coloc and
    // the nodes-coloc bundle) was removed. `coloc_abf` registers only
    // through the coloc manifest plugin when a plugins root is present;
    // this plugin-free registry must not carry it.
    assert!(!kinds.iter().any(|node| node.kind == "coloc_abf"));
}

#[test]
fn donor_composition_node_is_registered() {
    let registry = registry();

    assert!(
        registry
            .list_nodes()
            .iter()
            .any(|node| node.kind == "hypothesize.donor_composition_test")
    );
}
