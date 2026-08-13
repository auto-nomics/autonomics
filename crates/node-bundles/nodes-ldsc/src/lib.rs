//! LDSC and LCV DAG node bundle.

pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;

use std::collections::BTreeMap;

use dag_core::resource_catalog::{
    ResourceAddress, ResourceEntry, ResourceKind, ResourceProvider,
};
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "ldsc"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(ldsc_hsq::LdscHsqNodeFactory {}));
        registry.register(Box::new(ldsc_rg::LdscRgNodeFactory {}));
        registry.register(Box::new(ldsc_sldsc::LdscSldscNodeFactory {}));
        registry.register(Box::new(liability::LiabilityNodeFactory {}));
        registry.register(Box::new(lcv::LcvNodeFactory {}));
    }
}

// ── Resource metadata helpers ─────────────────────────────────────────────

/// Provenance and sizing fields shared by every LD-score panel entry.
///
/// Grouped into a struct to keep [`panel_metadata`] under clippy's
/// argument-count threshold.
struct PanelSpec<'a> {
    source_doi: &'a str,
    raw_archive: &'a str,
    parquet_archive: &'a str,
    population: &'a str,
    annotation_version: &'a str,
    n_annotations: usize,
    n_snps: usize,
    consumer_nodes: &'a str,
}

/// Build the common metadata map for an LD-score panel entry.
fn panel_metadata(spec: &PanelSpec<'_>, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("source".into(), spec.source_doi.into());
    m.insert("raw_archive".into(), spec.raw_archive.into());
    m.insert("parquet_archive".into(), spec.parquet_archive.into());
    m.insert("population".into(), spec.population.into());
    m.insert("reference_panel".into(), "1000G".into());
    m.insert("annotation_version".into(), spec.annotation_version.into());
    m.insert("n_annotations".into(), spec.n_annotations.to_string());
    m.insert("n_snps".into(), spec.n_snps.to_string());
    m.insert(
        "convert_script".into(),
        "/mnt/disk3/autonomics/infra/sink_ldsc_panel/convert_to_parquet.py".into(),
    );
    m.insert("consumer_nodes".into(), spec.consumer_nodes.into());
    for (k, v) in extra {
        m.insert((*k).into(), (*v).into());
    }
    m
}

// ── Resource declarations ─────────────────────────────────────────────────

/// Resource declarations for LDSC bundle: LD-score panels used by h², rg,
/// S-LDSC, LCV nodes. Each entry is self-describing — the `description` and
/// `metadata` fields together serve as the canonical documentation for the
/// panel, traceable through the resource catalog.
///
/// **Migration note (2026-08)**: every panel here is declared as
/// `ResourceKind::ObjectStorage`, replacing the legacy Iceberg table address.
/// Reads go through DataFusion's `ListingTable` over the engine's
/// `opendal`-backed object store (see `nodes_ldsc::ldsc_common::register_listing_table`).
/// The historical Iceberg path (`iceberg.ld_score.<table>`) is preserved in
/// `metadata.parquet_archive` for traceability.
pub struct Resources;
impl ResourceProvider for Resources {
    fn name(&self) -> &'static str {
        "ldsc"
    }
    fn resources(&self) -> Vec<ResourceEntry> {
        // OSS bucket shared by every LD-score panel. Read via the engine's
        // opendal `oss://` operator, registered against the runtime
        // `RuntimeEnv` at engine bootstrap.
        const PANEL_BUCKET: &str = "autonomics-data";

        vec![
            // ── Univariate panel (base annotation) ──────────────────────
            ResourceEntry::new(
                "ldscore.1000g_eur",
                ResourceKind::ObjectStorage,
                "1000G EUR univariate LD-score panel for LDSC h² and rg estimation. \
                 Contains a single ld_score column (the baseline 'baseL2' annotation = \
                 Σr² to all nearby SNPs within 1 cM) and a w_ld column (weight LD score \
                 from HapMap3 no-MHC SNPs). Used by the `ldsc` (h²) and `ldsc_rg` (rg) \
                 DAG nodes, which join GWAS sumstats on rsid and regress χ² onto ld_score. \
                 The companion `{table}_m` table provides M_5_50 (the L2-summed SNP count \
                 for MAF ∈ [0.05, 0.5]) used to normalise the regression slope into h². \
                 Source: Zenodo DOI 10.5281/zenodo.10515792 (baselineLD v2.2, 1000G EUR). \
                 Raw .l2.ldscore.gz → Parquet via convert_to_parquet.py. \
                 Schema: locus<contig:string, position:int32>, rsid:string, \
                 ld_score:double, w_ld:double. 1,187,349 SNPs (inner-join of baselineLD \
                 ∩ weights.hm3_noMHC across 22 autosomes).\
                 \n\n**Storage**: object-storage parquet partition — \
                 read via DataFusion `ListingTable` over the engine's opendal \
                 `oss://autonomics-data/ld_score/1000g_eur/` prefix.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/1000g_eur/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "baselineLD v2.2 (base annotation only)",
                    n_annotations: 1,
                    n_snps: 1_187_349,
                    consumer_nodes: "ldsc (h²), ldsc_rg (genetic correlation)",
                },
                &[
                    ("columns", "locus<contig,position>, rsid, ld_score, w_ld"),
                    ("m_5_50_base", "5961159"),
                    ("m_table", "iceberg.ld_score.1000g_eur_m"),
                    ("qc_filter", "inner-join baselineLD ∩ weights.hm3_noMHC; autosomes 1-22"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "1000g".into(),
                "eur".into(),
                "univariate".into(),
                "reference_panel".into(),
            ]),

            // ── Univariate M companion ──────────────────────────────────
            ResourceEntry::new(
                "ldscore.1000g_eur.m",
                ResourceKind::ObjectStorage,
                "M_5_50 companion table for the univariate 1000g_eur LD-score panel. \
                 Single row: annotation='baseL2', m_5_50=5,961,159 (Σ M_5_50 across \
                 22 autosomes). Read by ldsc_common::read_m_5_50() to normalise the \
                 LDSC regression slope (h² = slope × M / N). Using COUNT(*) instead of \
                 M_5_50 overestimates M and inflates h² proportionally.\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/1000g_eur_m/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/1000g_eur_m/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "baselineLD v2.2 (base annotation only)",
                    n_annotations: 1,
                    n_snps: 1, // M table has 1 row
                    consumer_nodes: "ldsc (h²), ldsc_rg (genetic correlation)",
                },
                &[
                    ("columns", "annotation:string, m_5_50:double"),
                    ("m_5_50_base", "5961159"),
                    ("panel_table", "iceberg.ld_score.1000g_eur"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "m_5_50".into(),
                "1000g".into(),
                "eur".into(),
                "companion".into(),
            ]),

            // ── Multi-annotation panel (baselineLD v2.2, 97 annotations) ─
            ResourceEntry::new(
                "ldscore.baselineLD_v2_2_eur",
                ResourceKind::ObjectStorage,
                "baselineLD v2.2 EUR 97-annotation LD-score panel for stratified LDSC \
                 (S-LDSC, partitioned heritability). Contains all 97 functional annotation \
                 LD-score columns (e.g. baseL2, Coding_UCSCL2, Conserved_LindbladTohL2, \
                 DHS_peaks_TrynkaL2, ...) plus a w_ld column. Used by the `sldsc` DAG \
                 node, which discovers annotation names from the companion M table, joins \
                 GWAS sumstats on rsid, and regresses χ² onto the 97-column LD-score design \
                 matrix to partition h² across functional categories. \
                 Source: Zenodo DOI 10.5281/zenodo.10515792 (Gazart 2024, baselineLD v2.2, \
                 1000G EUR). Annotation column names preserve the 'L2' suffix from the \
                 baselineLD header. The companion table provides per-annotation M_5_50 \
                 (97 rows). \
                 Schema: locus<contig:string, position:int32>, rsid:string, \
                 97 × {annotation}L2:double, w_ld:double. 1,187,349 SNPs.\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/baselineLD_v2_2_eur/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/baselineLD_v2_2_eur/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "baselineLD v2.2 (97 annotations)",
                    n_annotations: 97,
                    n_snps: 1_187_349,
                    consumer_nodes: "sldsc (partitioned / stratified heritability)",
                },
                &[
                    ("columns", "locus<contig,position>, rsid, 97×{annot}L2:double, w_ld"),
                    ("annotation_categories", "base, Coding_UCSC, Conserved, CTCF, DGF, DHS, Enhancer, Promoter, SuperEnhancer, TFBS, TSS, ..."),
                    ("m_table", "iceberg.ld_score.baselineLD_v2_2_eur_m"),
                    ("qc_filter", "inner-join baselineLD ∩ weights.hm3_noMHC; autosomes 1-22"),
                    ("annotation_naming", "column names end with 'L2' suffix (e.g. baseL2, Coding_UCSCL2)"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "1000g".into(),
                "eur".into(),
                "baselineLD".into(),
                "v2.2".into(),
                "multi_annotation".into(),
                "reference_panel".into(),
            ]),

            // ── Multi-annotation M companion ────────────────────────────
            ResourceEntry::new(
                "ldscore.baselineLD_v2_2_eur.m",
                ResourceKind::ObjectStorage,
                "M_5_50 companion table for the baselineLD v2.2 multi-annotation panel. \
                 97 rows, one per annotation. Read by ldsc_common::read_m_5_50() and the \
                 sldsc node to derive per-annotation SNP proportions, enrichment, and \
                 partitioned h². The annotation column carries the full annotation name \
                 (with 'L2' suffix); m_5_50 is the L2-summed SNP count for MAF ∈ [0.05, 0.5] \
                 summed across 22 autosomes.\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/baselineLD_v2_2_eur_m/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/baselineLD_v2_2_eur_m/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "baselineLD v2.2 (97 annotations)",
                    n_annotations: 97,
                    n_snps: 97, // M table has 97 rows
                    consumer_nodes: "sldsc (partitioned / stratified heritability)",
                },
                &[
                    ("columns", "annotation:string, m_5_50:double"),
                    ("panel_table", "iceberg.ld_score.baselineLD_v2_2_eur"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "m_5_50".into(),
                "1000g".into(),
                "eur".into(),
                "baselineLD".into(),
                "companion".into(),
            ]),

            // ── Frequency table (1000G EUR allele frequencies) ───────────
            ResourceEntry::new(
                "ldscore.1000g_eur_frq",
                ResourceKind::ObjectStorage,
                "1000G EUR allele frequency table for S-LDSC QC and MAF filtering. \
                 One row per SNP per chromosome (MAF ≥ 1%, QC-passed). Used to verify \
                 MAF ranges, compute M_5_50 companion counts, and align alleles between \
                 GWAS sumstats and the reference panel. \
                 Schema: chr:int32, rsid:string, a1:string, a2:string, maf:double, \
                 nchrobs:int32. 4,501,760 SNPs across 22 autosomes. \
                 Source: Zenodo DOI 10.5281/zenodo.10515792 (1000G Phase 3 EUR QC).\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/1000g_eur_frq/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/1000g_eur_frq/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "1000G Phase 3 QC frequencies",
                    n_annotations: 0, // not annotations
                    n_snps: 4_501_760,
                    consumer_nodes: "sldsc (MAF QC), ldsc (allele alignment)",
                },
                &[
                    ("columns", "chr:int32, rsid:string, a1:string, a2:string, maf:double, nchrobs:int32"),
                    ("qc_filter", "MAF >= 0.01; 18 malformed rows (missing A2/MAF) dropped"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "frequency".into(),
                "1000g".into(),
                "eur".into(),
                "reference_panel".into(),
            ]),

            // ── Annotation matrix (baselineLD v2.2, 97 annotations per SNP) ─
            ResourceEntry::new(
                "ldscore.baselineLD_v2_2_eur_annot",
                ResourceKind::ObjectStorage,
                "baselineLD v2.2 EUR raw annotation matrix — 97 functional annotations \
                 per SNP. Each row carries 0/1 binary indicators (or continuous values \
                 for allele-frequency bin annotations) for Coding, Conserved, CTCF, DGF, \
                 DHS, Enhancer, Promoter, SuperEnhancer, TFBS, TSS and flanking categories. \
                 Used to understand SNP-to-annotation membership, compute custom LD-score \
                 annotations, and derive enrichment analyses. \
                 Schema: locus<contig,position>, rsid, 97 annotation columns (double). \
                 ~10M SNPs. Source: Zenodo DOI 10.5281/zenodo.10515792 (baselineLD v2.2, 1000G EUR).\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/baselineLD_v2_2_eur_annot/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/baselineLD_v2_2_eur_annot/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "baselineLD v2.2 raw annotation matrix",
                    n_annotations: 97,
                    n_snps: 9_997_231,
                    consumer_nodes: "sldsc (annotation enrichment), custom annotation pipeline",
                },
                &[
                    ("columns", "locus<contig,position>, rsid, 97×annotation:double"),
                    ("annotation_categories",
                     "base, Coding_UCSC, Conserved_LindbladToh, CTCF_Hoffman, DGF_ENCODE, \
                      DHS_peaks_Trynka, Enhancer_Hoffman, PromoterFlanking_Hoffman, \
                      SuperEnhancer_Hnisz, TFBS_ENCODE, TSS, ..."),
                    ("n_chromosomes", "22"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "annotation".into(),
                "baselineLD".into(),
                "1000g".into(),
                "eur".into(),
                "reference_panel".into(),
            ]),

            // ── HapMap3 SNP inclusion list (no MHC) ──────────────────────
            ResourceEntry::new(
                "ldscore.hm3_no_mhc",
                ResourceKind::ObjectStorage,
                "HapMap3 SNP inclusion list (no MHC region). 1,217,311 SNPs used as the \
                 reference backbone for LDSC/S-LDSC analysis. GWAS summary statistics are \
                 filtered to this SNP set before regression to ensure consistent SNP \
                 coverage across traits. The MHC region (chr6, ~25-35 Mb) is excluded \
                 due to its complex LD structure. \
                 Schema: rsid:string only. Source: Zenodo DOI 10.5281/zenodo.10515792.\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/hm3_no_mhc/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/hm3_no_mhc/"),
            )
            .with_metadata(panel_metadata(
                &PanelSpec {
                    source_doi: "https://zenodo.org/records/10515792",
                    raw_archive: "aliyun:autonomics-data/ldsc/s-ldsc-ref/",
                    parquet_archive: "aliyun:autonomics-data/ldsc/panels/",
                    population: "EUR",
                    annotation_version: "HapMap3 no-MHC backbone",
                    n_annotations: 0,
                    n_snps: 1_217_311,
                    consumer_nodes: "ldsc, ldsc_rg, sldsc (SNP filtering backbone)",
                },
                &[
                    ("columns", "rsid:string"),
                    ("exclusion", "MHC region (chr6:25-35Mb)"),
                ],
            ))
            .with_tags(vec![
                "ld_score".into(),
                "hm3".into(),
                "snp_list".into(),
                "reference_panel".into(),
            ]),

            // ── Legacy UKBB panel (declared, not yet ingested) ──────────
            ResourceEntry::new(
                "ldscore.ukbb_eur",
                ResourceKind::ObjectStorage,
                "UKBB EUR LD-score panel (single ld_score column, no w_ld). Legacy panel \
                 from the UK Biobank EUR cohort. Not yet ingested into the production \
                 lake; nodes that reference it fall back to this declaration but will \
                 fail at query time until data is loaded.\
                 \n\n**Storage**: object-storage parquet partition — \
                 `oss://autonomics-data/ld_score/ukbb_eur/`.",
                ResourceAddress::object_storage(PANEL_BUCKET, "/ld_score/ukbb_eur/"),
            )
            .with_tags(vec![
                "ld_score".into(),
                "ukbb".into(),
                "eur".into(),
                "legacy".into(),
                "not_ingested".into(),
            ]),
        ]
    }
}
