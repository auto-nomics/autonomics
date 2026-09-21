//! Built-in runtime bundle mappings.
//!
//! These entries make registry-backed integration tests and embedded engines
//! buildable without copying a catalog into every test. Runtime entries from
//! `data_bundles.toml` always override these defaults.

use std::sync::Arc;

use dag_core::{BundleRegistry, DataBundle};

fn bundle(id: &str, desc: &str, vpath: &str) -> DataBundle {
    DataBundle::new(id, desc, vpath)
}

/// Build the registry of bundles embedded with the data engine.
///
/// These mappings keep registry-backed tests and embedded engines usable
/// before a Hugging Face catalog package has been installed.
pub fn builtin_bundle_registry() -> BundleRegistry {
    let mut bundles = Vec::new();
    bundles.extend([
        bundle(
            "ldscore.1000g_eur",
            "1000G EUR LD Score panel",
            "/bundles/ldsc/1000g_eur.parquet",
        ),
        bundle(
            "ldscore.1000g_eur_m",
            "1000G EUR LD Score M panel",
            "/bundles/ldsc/1000g_eur_m.parquet",
        ),
        bundle(
            "ldscore.baselineLD_v2_2_eur",
            "baselineLD v2.2 EUR LD Score panel",
            "/bundles/ldsc/baselineLD_v2_2_eur.parquet",
        ),
        bundle(
            "ldscore.baselineLD_v2_2_eur_m",
            "baselineLD v2.2 EUR LD Score M panel",
            "/bundles/ldsc/baselineLD_v2_2_eur_m.parquet",
        ),
        bundle(
            "ldscore.ukbb_eur",
            "UKBB EUR LD Score panel",
            "/bundles/ldsc/ukbb_eur.parquet",
        ),
        bundle(
            "ldmatrix.1000g_eur",
            "1000G EUR LD matrix chromosome template",
            "/bundles/ldmatrix/1000g_eur/chr{N}",
        ),
        bundle(
            "mixer.g1000_eur",
            "MiXeR 1000G EUR engine and signed-LD bundle",
            "/bundles/mixer/g1000_eur",
        ),
        bundle(
            "g1000_eur",
            "MAGMA 1000G EUR reference panel",
            "/bundles/magma/g1000_eur",
        ),
        bundle(
            "magma.gene_loc",
            "MAGMA NCBI gene-location table",
            "/bundles/magma/gene_loc.parquet",
        ),
        bundle(
            "kegg.genes",
            "KEGG entity-gene table",
            "/bundles/kegg/genes.parquet",
        ),
        bundle(
            "kegg.pathway_ko",
            "KEGG pathway-KO table",
            "/bundles/kegg/pathway_ko.parquet",
        ),
        bundle(
            "kegg.pathways",
            "KEGG pathway table",
            "/bundles/kegg/pathways.parquet",
        ),
        bundle(
            "kegg.genome_pathways",
            "KEGG genome-pathway table",
            "/bundles/kegg/genome_pathways.parquet",
        ),
        bundle(
            "als_cns.cell_markers",
            "ALS / CNS cell markers (astrocyte, microglia, oligodendrocyte, neuron) from CellMarker 2.0",
            "/bundles/als_cns.cell_markers/markers.tsv",
        ),
    ]);
    for population in ["afr", "amr", "eas", "sas"] {
        bundles.push(bundle(
            &format!("g1000_{population}"),
            &format!("MAGMA 1000G {} reference panel", population.to_uppercase()),
            &format!("/bundles/magma/g1000_{population}"),
        ));
    }
    BundleRegistry::from_bundles(bundles)
        .expect("built-in data bundle IDs are unique and use absolute vpaths")
}

/// Layer user-configured bundles on top of the built-in registry.
///
/// Returns an `Arc` because the merged projection is installed in both the
/// engine context and every node-registry context.
pub fn registry_with_builtins(user: &BundleRegistry) -> Arc<BundleRegistry> {
    let overrides: Vec<DataBundle> = user.iter().map(|(_, bundle)| bundle.clone()).collect();
    let merged = builtin_bundle_registry()
        .with_overriding_bundles(overrides)
        .expect("runtime data bundles should be validated when loaded");
    Arc::new(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_contains_global_ldsc_bundles() {
        let registry = builtin_bundle_registry();

        assert!(registry.get("ldscore.1000g_eur").is_some());
        assert!(registry.get("ldscore.1000g_eur_m").is_some());
        assert!(registry.get("ldscore.baselineLD_v2_2_eur").is_some());
        assert_eq!(
            registry
                .get("als_cns.cell_markers")
                .map(|bundle| bundle.vpath.as_str()),
            Some("/bundles/als_cns.cell_markers/markers.tsv")
        );
    }

    #[test]
    fn runtime_entries_override_builtin_entries() {
        let mut user = BundleRegistry::new();
        user.register(DataBundle::new(
            "ldscore.1000g_eur",
            "Runtime override",
            "/runtime/ldscore.parquet",
        ))
        .unwrap();

        let merged = registry_with_builtins(&user);

        assert_eq!(
            merged
                .get("ldscore.1000g_eur")
                .map(|bundle| bundle.vpath.as_str()),
            Some("/runtime/ldscore.parquet")
        );
    }
}
