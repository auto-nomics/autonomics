//! Wave end-to-end: every published plugin family installs from its
//! pinned git rev, loads, and registers its kinds.
//!
//! Requires network access to the pushed plugin repositories; run
//! explicitly: `cargo test -p container-plugin --test plugin_wave_e2e
//! -- --ignored`

use std::sync::Arc;

use dag_core::NodePlugin;
use dag_core::registry::NodeRegistry;

/// (plugin name, git url, pinned rev, expected kinds)
const FAMILIES: &[(&str, &str, &str, &[&str])] = &[
    (
        "ldsc",
        "git@github.com:auto-nomics/ldsc-plugin.git",
        "6f7118d61dd60ca7ce95d7d524ccec3880d96026",
        &["ldsc_h2", "ldsc_munge", "ldsc_rg"],
    ),
    (
        "magma",
        "git@github.com:auto-nomics/magma-plugin.git",
        "996608a0b4d41c3c0ecdb030610064c7a3366d59",
        &["magma_annotate"],
    ),
    (
        "mrpresso",
        "git@github.com:auto-nomics/mrpresso-plugin.git",
        "670007a7432b0e9494797e5b12b10010bfc74b18",
        &["mrpresso"],
    ),
    (
        "mvmr",
        "git@github.com:auto-nomics/mvmr-plugin.git",
        "ab8bfe0a7b5f484ce4264a7b8a87e25d7f66da23",
        &["mvmr"],
    ),
    (
        "coloc",
        "git@github.com:auto-nomics/coloc-plugin.git",
        "39795c4cd91febb7ca0a3ff2bcdf449c75e09042",
        &["coloc_abf"],
    ),
    (
        "deseq2",
        "git@github.com:auto-nomics/deseq2-plugin.git",
        "73581fb4414e149b72b60205418289de1bb08f65",
        &["deseq2_de"],
    ),
    (
        "gcta",
        "git@github.com:auto-nomics/gcta-plugin.git",
        "401479d95c9442387ece637f589d448f4eddbbc2",
        &[
            "gcta_cojo_select",
            "gcta_sblup",
            "gcta_fastbat",
            "gcta_acat",
        ],
    ),
    (
        "pathway-gsea",
        "git@github.com:auto-nomics/pathway-gsea-plugin.git",
        "3121a9070d4f57c0fa92d03c90abd31dbe4a536e",
        &["pathway_gsea"],
    ),
    (
        "plink2",
        "git@github.com:auto-nomics/plink2-plugin.git",
        "a72fe6f6839a8c85137c69499d87a72b58bf0f61",
        &["plink2_clump"],
    ),
    (
        "visualization",
        "git@github.com:auto-nomics/visualization-plugin.git",
        "aab8ef00e480883e037dff0f0cebc9024a496ad3",
        &["visualization"],
    ),
    (
        "mtag",
        "git@github.com:auto-nomics/mtag-plugin.git",
        "0d25b1fe3e5694a6dc7cc92777040229f3d442b7",
        &["mtag"],
    ),
    (
        "smr",
        "git@github.com:auto-nomics/smr-plugin.git",
        "83c058249cd5104b4361e546c2f3292abd0c2272",
        &["smr_heidi", "smr_heidi_eqtlgen"],
    ),
    (
        "susie",
        "git@github.com:auto-nomics/susie-plugin.git",
        "7d45f9918d16c1307d52f6f6d3c14df2debe682e",
        &["susie_rss"],
    ),
    (
        "twosamplemr",
        "git@github.com:auto-nomics/twosamplemr-plugin.git",
        "1d35db811cd0516b92ea52ea6b21233a3b5bd6da",
        &["twosamplemr", "twosamplemr_harmonise"],
    ),
    (
        "mixer",
        "git@github.com:auto-nomics/mixer-plugin.git",
        "eb495e492c4f5a47605643a6132792c0700d6ec8",
        &["mixer_fit1", "mixer_fit2"],
    ),
    (
        "music",
        "git@github.com:auto-nomics/music-plugin.git",
        "63dd223fb2c691897f7ae5c6c8a96c7128a75a71",
        &["music_deconvolution"],
    ),
    (
        "mutation",
        "git@github.com:auto-nomics/mutation-plugin.git",
        "f2a09e5fd8172d888fc0821ec693ee746a1f0669",
        &["mutation_analysis", "mutation_analysis_clinical"],
    ),
    (
        "timesfm",
        "git@github.com:auto-nomics/timesfm-plugin.git",
        "a0a2b518ff01f8f4a4a815a0f27c2d07240a2272",
        &["timesfm_forecast"],
    ),
    (
        "twas",
        "git@github.com:auto-nomics/twas-plugin.git",
        "bab7c16eae62fd19d8e66a944d513f78d7872c3d",
        &["twas_fusion"],
    ),
    (
        "hdl",
        "git@github.com:auto-nomics/hdl-plugin.git",
        "b97fe56a5dc8db93d35c25c88c6b2be8b0965a7d",
        &["hdl_l", "hdl_l_scan"],
    ),
    (
        "lava",
        "git@github.com:auto-nomics/lava-plugin.git",
        "2a83031a0776a193d845381131b5ba7e9e5f65a9",
        &["lava", "lava_scan"],
    ),
    (
        "single-cell",
        "git@github.com:auto-nomics/single-cell-plugin.git",
        "1d681eac33fe7fbde125a67924284270d737b6b2",
        &[
            "single_cell_preprocessor",
            "h5ad_pca_neighbors_umap_leiden",
            "h5ad_celltypist_annotate",
            "h5ad_subset_by_obs",
            "sc_dense_ingest",
            "h5ad_rank_genes_groups",
            "h5ad_cluster_mean_expression",
            "gene_set_score",
            "h5ad_marker_annotate",
            "h5ad_ucell_score",
        ],
    ),
    (
        "radiomics",
        "git@github.com:auto-nomics/radiomics-plugin.git",
        "a6a7d659d6b702572b77c5390f54ff2da768ce85",
        &[
            "radiomics_image_ingest",
            "radiomics_mask_ingest",
            "radiomics_pair_validate",
            "radiomics_preprocess",
            "pyradiomics_extract",
            "pyradiomics_batch_extract",
            "radiomics_dicom_metadata",
            "radiomics_phi_scrub",
            "radiomics_voi_dice_hausdorff",
            "radiomics_image_qc",
            "radiomics_rtstruct_geometry",
            "radiomics_ivh_extract",
            "radiomics_shape_topology",
            "radiomics_register",
            "radiomics_delta_features",
            "radiomics_bias_correct",
            "radiomics_robust_normalize",
            "radiomics_peritumoral_ring",
            "radiomics_habitat_fit",
            "radiomics_habitat_assign",
            "radiomics_perturb_stability",
        ],
    ),
    (
        "pathology",
        "git@github.com:auto-nomics/pathology-plugin.git",
        "641fe464d478f07235657a962db791c5603d9ab9",
        &[
            "pathology_wsi_ingest",
            "pathology_wsi_qc",
            "pathology_patch_sample",
            "pathology_wsi_embed",
            "pathology_domain_check",
            "pathology_ihc_quant",
            "pathology_qupath_import",
        ],
    ),
    (
        "bulk-rnaseq",
        "https://github.com/auto-nomics/bulk-rnaseq-plugin.git",
        "178e71c7cc8d82f9f4bacd16f009530c7813e717",
        &["limma_voom", "wgcna"],
    ),
    (
        "hyprcoloc",
        "https://github.com/auto-nomics/hyprcoloc-plugin.git",
        "c876b5a55ad2a6258b1304e827d6a92bfe1cf7a6",
        &["hyprcoloc"],
    ),
];

#[test]
#[ignore = "requires network access to clone the pushed plugin repositories"]
fn all_wave_families_install_from_git_and_register() {
    let state = tempfile::tempdir().unwrap();
    let mut config = String::from("\n");
    for (name, url, rev, _) in FAMILIES {
        config.push_str(&format!(
            "[[plugin]]\nname = \"{name}\"\ngit = \"{url}\"\nrev = \"{rev}\"\n\n"
        ));
    }
    let config_path = state.path().join("plugins.toml");
    std::fs::write(&config_path, config).unwrap();
    let root = state.path().join("plugins");

    // 1. Install all five families at their pinned revs.
    let report = container_plugin::sync::sync(&config_path, &root).unwrap();
    assert_eq!(report.outcomes.len(), FAMILIES.len());
    for (name, outcome) in &report.outcomes {
        assert_eq!(
            *outcome,
            container_plugin::sync::EntryOutcome::Installed,
            "{name}"
        );
    }

    // 2. Load: one Plugin per family, all kinds present.
    let runtime: Arc<dyn container_runtime::PodmanConnection> = Arc::new(
        container_runtime::PodmanRuntime::new(container_runtime::PodmanConfig {
            program: "podman".into(),
            workspace_root: root.join("workspace"),
            panel_cache_root: root.join("panels"),
        }),
    );
    let panel_cache = Arc::new(container_runtime::PanelCache::new(root.join("panels")));
    let plugins = container_plugin::loader::load(&root, runtime, panel_cache).unwrap();
    assert_eq!(plugins.len(), FAMILIES.len());

    let ctx = dag_core::NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        None,
    );
    let mut registry = NodeRegistry::new(ctx);
    for plugin in &plugins {
        registry.register_plugin(plugin);
    }

    let registered: Vec<String> = registry
        .list_nodes()
        .into_iter()
        .map(|node| node.kind)
        .collect();
    for (name, _, _, kinds) in FAMILIES {
        for kind in *kinds {
            assert!(
                registered.iter().any(|k| k == kind),
                "plugin `{name}` kind `{kind}` missing; registered: {registered:?}"
            );
        }
    }
}
