//! Registry end-to-end checks for every node kind with runtime bundle bindings.

use std::sync::Arc;

use dag_core::{DataBundle, DataBundleCatalog};
use data_engine::data_engine::DataEngine;
use datafusion::prelude::SessionContext;

fn bundle(id: &str, vpath: &str) -> DataBundle {
    DataBundle::new(id, id, vpath)
}

fn catalog_panel(id: &str, source: &str) -> DataBundle {
    let mut value = DataBundle::new(id, id, format!("/bundles/{id}"));
    value.source = Some(source.into());
    value.digest = Some(format!("sha256:{}", id.len()));
    value
}

fn catalog() -> DataBundleCatalog {
    DataBundleCatalog::from_bundles([
        catalog_panel(
            nodes_io::ldsc_h2_container::LDSC_REF_LD_PANEL,
            "/catalog/ref-ld",
        ),
        catalog_panel(
            nodes_io::ldsc_h2_container::LDSC_W_LD_PANEL,
            "/catalog/w-ld",
        ),
        catalog_panel(
            nodes_io::magma_annotate_container::MAGMA_GENE_LOC_PANEL,
            "/catalog/magma-gene-loc",
        ),
        catalog_panel(
            nodes_io::lava_container::LAVA_TUTORIAL_REF_PANEL,
            "/catalog/lava-ref",
        ),
        catalog_panel(
            nodes_io::lava_container::LAVA_UKB_EUR_PANEL,
            "/catalog/lava-ref-ukb",
        ),
        catalog_panel(
            nodes_io::mixer_container::MIXER_G1000_EUR_PANEL,
            "/catalog/mixer-g1000-eur",
        ),
        catalog_panel(
            nodes_io::gcta_container::GCTA_REF_BINARY_PANEL,
            "/catalog/gcta-plink-ref",
        ),
        catalog_panel(
            nodes_io::gcta_container::GCTA_GENE_LIST_PANEL,
            "/catalog/gcta-gene-list",
        ),
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
        bundle("ldmatrix.1000g_eur", "/bundles/ldmatrix/chr{N}"),
        bundle("g1000_eur", "/bundles/magma/g1000_eur"),
        bundle("plink.1000g_eur", "/bundles/plink/chr{N}/panel"),
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
    data_engine::default_registry::build_default_registry(
        ctx.runtime_env(),
        None,
        Arc::new(catalog()),
    )
}

#[test]
fn all_bundle_bound_node_kinds_build_from_runtime_catalog() {
    let cases = [
        (
            "file_ref_source",
            serde_json::json!({
                "path": "/inputs/example.sumstats.gz",
                "format": "sumstats_gz"
            }),
        ),
        (
            "ldsc_h2_container",
            serde_json::json!({"n_blocks": 5, "intercept_h2": null}),
        ),
        (
            "ldsc_munge_container",
            serde_json::json!({
                "snp": "variant",
                "p": "pval",
                "signed_sumstats": {"column": "beta", "null_value": 0.0},
                "n": 100000.0
            }),
        ),
        ("ldsc_rg_container", serde_json::json!({"n_blocks": 5})),
        ("magma_annotate_container", serde_json::json!({})),
        (
            "mrpresso_container",
            serde_json::json!({
                "beta_outcome": "Y_effect",
                "sd_outcome": "Y_se",
                "beta_exposure": ["E1_effect"],
                "sd_exposure": ["E1_se"]
            }),
        ),
        (
            "lava_container",
            serde_json::json!({
                "analysis": "univ",
                "panel_id": "lava.ref.ukb_eur",
                "phenotypes": ["bmi"]
            }),
        ),
        (
            "mixer_fit1_container",
            serde_json::json!({
                "chr2use": "21-22",
                "fast_run": true
            }),
        ),
        (
            "mixer_fit2_container",
            serde_json::json!({
                "chr2use": "21-22",
                "fast_run": true
            }),
        ),
        (
            "mvmr_container",
            serde_json::json!({
                "beta_yg": "SBP_beta",
                "sebeta_yg": "SBP_se",
                "beta_xg": ["LDL_beta", "HDL_beta"],
                "sebeta_xg": ["LDL_se", "HDL_se"],
                "label_column": "SNP"
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
        ("mtag", serde_json::json!({"n_blocks": 5})),
        ("gsem_ldsc", serde_json::json!({"n_traits": 2})),
        ("magma_gene", serde_json::json!({"population": "EUR"})),
        (
            "magma_kegg_align",
            serde_json::json!({"min_set_size": 1, "max_set_size": 10}),
        ),
        ("susie_rss_container", serde_json::json!({})),
        (
            "gcta_cojo_select_container",
            serde_json::json!({"chr": 22, "cojo_p": 0.05}),
        ),
        (
            "gcta_sblup_container",
            serde_json::json!({"chr": 22, "lambda": 1.33e6}),
        ),
        ("gcta_fastbat_container", serde_json::json!({"chr": 22})),
        (
            "gcta_acat_container",
            serde_json::json!({"max_maf": 0.05, "min_mac": 1}),
        ),
        (
            "hdl_l",
            serde_json::json!({
                "chr": 1,
                "start": 1,
                "stop": 1000,
                "trait1_name": "a",
                "trait2_name": "b"
            }),
        ),
        (
            "hdl_l_scan",
            serde_json::json!({
                "chr": 1,
                "scan_start": 1,
                "scan_stop": 1000,
                "window_size": 500,
                "step": 500,
                "trait1_name": "a",
                "trait2_name": "b"
            }),
        ),
        (
            "two_sample_mr",
            serde_json::json!({
                "id_exposure": "exposure",
                "id_outcome": "outcome",
                "clump": {"type": "local_ld"}
            }),
        ),
    ];
    let registry = registry();

    for (kind, spec) in cases {
        registry
            .build_node(kind, spec)
            .unwrap_or_else(|error| panic!("build `{kind}` through bundle catalog: {error}"));
    }
}

#[test]
fn missing_catalog_bundle_rejects_node_build() {
    let ctx = SessionContext::new();
    let registry = data_engine::default_registry::build_default_registry(
        ctx.runtime_env(),
        None,
        Arc::new(DataBundleCatalog::new()),
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
        Arc::new(DataBundleCatalog::new()),
    );

    registry
        .build_node("sldsc", serde_json::json!({}))
        .unwrap_or_else(|error| panic!("build `sldsc` from built-in catalog: {error}"));
}

#[test]
fn default_data_engine_uses_builtin_bundle_catalog() {
    let mut engine = DataEngine::builder().build();

    engine
        .add_node_from_registry("sldsc", "sldsc", serde_json::json!({}))
        .unwrap();
}

#[test]
fn node_listing_exposes_static_bundle_requirements() {
    let registry = registry();
    let ldsc = registry
        .list_nodes()
        .into_iter()
        .find(|node| node.kind == "ldsc_h2_container")
        .unwrap();

    assert_eq!(ldsc.data_bundles.len(), 2);
}
