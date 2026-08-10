//! Statistical genetics DAG node bundle.

pub mod bivariate_mixer;
pub mod cpassoc;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod lava;
pub mod magma;
pub mod mtag;
pub mod susie_rss;
pub mod univariate_mixer;

use std::collections::BTreeMap;

use dag_core::resource_catalog::{
    ResourceAddress, ResourceEntry, ResourceKind, ResourceProvider, CATALOG_NAME,
};
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(lava::LavaLocusNodeFactory {}));
        registry.register(Box::new(lava::LavaUnivNodeFactory {}));
        registry.register(Box::new(lava::LavaBivarNodeFactory {}));
        registry.register(Box::new(lava::LavaPcorNodeFactory {}));
        registry.register(Box::new(lava::LavaMultiregNodeFactory {}));
        registry.register(Box::new(hdl_l::HdlLNodeFactory {}));
        registry.register(Box::new(hdl_l_scan::HdlLScanNodeFactory {}));
        registry.register(Box::new(mtag::MtagNodeFactory {}));
        registry.register(Box::new(cpassoc::CpassocNodeFactory {}));
        registry.register(Box::new(susie_rss::SusieRssNodeFactory {}));
        registry.register(Box::new(magma::MagmaAnnotateNodeFactory {}));
        registry.register(Box::new(magma::MagmaGeneNodeFactory {}));
        registry.register(Box::new(magma::MagmaSetNodeFactory {}));
        registry.register(Box::new(magma::MagmaMetaNodeFactory {}));
        registry.register(Box::new(univariate_mixer::UnivariateMixerNodeFactory {}));
        registry.register(Box::new(bivariate_mixer::BivariateMixerNodeFactory {}));
    }
}

// ── Resource metadata helpers ─────────────────────────────────────────────

/// Build the common metadata map for the LD-matrix entry.
fn ld_matrix_metadata() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("source_pipeline".into(), "PLINK2 --r2 from 1000G Phase 3 EUR VCF".into());
    m.insert("source_script".into(), "infra/thousand_genomes/ld_matrix.sh".into());
    m.insert("ingest_tool".into(), "infra/sink_ld_matrix (Rust binary)".into());
    m.insert("population".into(), "EUR".into());
    m.insert("reference_panel".into(), "1000G Phase 3".into());
    m.insert("maf_min".into(), "0.01".into());
    m.insert("geno_max".into(), "0.05".into());
    m.insert("ld_window_kb".into(), "10000".into());
    m.insert("ld_r2_min".into(), "0.01".into());
    m.insert("n_chromosomes".into(), "22".into());
    m.insert("columns".into(),
        "chrom_a:int64, pos_a:int64, id_a:string, chrom_b:int64, pos_b:int64, id_b:string, unphased_r2:float64".into());
    m.insert("consumer_nodes".into(),
        "susie_rss (LD correlation), univariate_mixer (tag selection + sufficient stats), \
         bivariate_mixer (tag selection), hdl_l_scan (local rg), two_sample_mr (IcebergLd clumping), \
         precompute_tags (tag panel + subgraph)".into());
    m.insert("table_pattern".into(), "iceberg.ld_matrix.eur_chr{1-22}".into());
    m.insert("docs".into(), "docs/data_infra/ld_matrix.md".into());
    m
}

// ── Resource declarations ─────────────────────────────────────────────────

/// Resource declarations for genetics bundle: LD-score panel (MTAG) and
/// per-chromosome LD-matrix tables (SuSiE-RSS, MiXeR, HDL-L, LAVA).
pub struct Resources;
impl ResourceProvider for Resources {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn resources(&self) -> Vec<ResourceEntry> {
        vec![
            // ── UKBB LD-score panel (MTAG) ────────────────────────────────
            ResourceEntry::new(
                "ldscore.ukbb_eur",
                ResourceKind::IcebergTable,
                "UKBB EUR LD-score panel (used by MTAG). Legacy panel from the UK Biobank \
                 EUR cohort. Not yet ingested into the production lake; nodes that reference \
                 it fall back to this declaration but will fail at query time until data \
                 is loaded.",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "ukbb_eur"),
            )
            .with_tags(vec![
                "ld_score".into(),
                "ukbb".into(),
                "eur".into(),
                "legacy".into(),
                "not_ingested".into(),
            ]),

            // ── LD-matrix (22 per-chromosome tables) ──────────────────────
            // This is the canonical rich declaration for the LD matrix.
            // The same entry is also declared (with minimal metadata) in
            // nodes-mr::Resources — idempotent registration means only this
            // version takes effect since genetics registers first.
            ResourceEntry::new(
                "ldmatrix.eur_chr",
                ResourceKind::IcebergTable,
                "1000G EUR pairwise LD-matrix: one Iceberg table per chromosome \
                 (eur_chr1 … eur_chr22). Each row is an LD pair (SNP A, SNP B) with \
                 unphased r² ≥ 0.01. Sparse storage — only meaningful LD pairs are \
                 kept (r² < 0.01 omitted). Built via PLINK2 --r2 from 1000G Phase 3 \
                 EUR VCF with QC (MAF ≥ 1%, geno ≤ 5%), 10 Mb window. Used by SuSiE-RSS \
                 (correlation matrix), MiXeR (tag selection + sufficient statistics), \
                 HDL-L scan (local rg), TwoSampleMR (IcebergLd clumping), and \
                 precompute_tags. Schema: chrom_a, pos_a, id_a, chrom_b, pos_b, id_b, \
                 unphased_r2. SNPs identified by rsid strings — integer indices are \
                 built at runtime by consumers, not persisted.",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_matrix", "eur_chr"),
            )
            .with_metadata(ld_matrix_metadata())
            .with_tags(vec![
                "ld_matrix".into(),
                "1000g".into(),
                "eur".into(),
                "pairwise_r2".into(),
                "sparse".into(),
                "reference_panel".into(),
            ]),

            // ── PLINK reference genotypes ─────────────────────────────────
            ResourceEntry::new(
                "plink.1000g_eur.ref_prefix",
                ResourceKind::FilePath,
                "1000G EUR PLINK reference genotypes (per-chromosome .bed/.bim/.fam). \
                 QC-filtered (MAF ≥ 1%, geno ≤ 5%) used as input for PLINK2 --r2 LD \
                 matrix computation. Pattern: {N} = chromosome number 1-22. \
                 Located at /mnt/disk2/dataset/1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc{.bed,.bim,.fam}.",
                ResourceAddress::path("/mnt/disk2/dataset/1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc"),
            )
            .with_metadata({
                let mut m = BTreeMap::new();
                m.insert("format".into(), "PLINK bed/bim/fam".into());
                m.insert("population".into(), "EUR".into());
                m.insert("reference_panel".into(), "1000G Phase 3".into());
                m.insert("qc".into(), "MAF >= 0.01, geno <= 0.05".into());
                m.insert("consumer".into(), "PLINK2 --r2 (LD matrix computation)".into());
                m
            })
            .with_tags(vec![
                "plink".into(),
                "1000g".into(),
                "eur".into(),
                "genotypes".into(),
                "reference_panel".into(),
            ]),
        ]
    }
}
