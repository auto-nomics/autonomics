//! GWAS Catalog REST API (curated catalog) — types and client methods.
//!
//! <https://www.ebi.ac.uk/gwas/docs/api>
//!
//! Spring Data REST backend. HAL JSON with `_embedded` and `page` metadata.
//! Pagination via `page` (0-indexed), `size`, `sort`.

use serde::Deserialize;

use crate::client::GwasCatalogClient;
use crate::error::Result;
use crate::summary_stats::HalLinks;

// ── Pagination envelope ─────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageStats {
    #[serde(default)]
    pub size: u32,
    #[serde(default)]
    pub total_elements: u64,
    #[serde(default)]
    pub total_pages: u32,
    #[serde(default)]
    pub number: u32,
}

/// Spring Data REST paginated collection wrapper.
#[derive(Debug, Deserialize)]
pub struct RestPage<T: serde::de::DeserializeOwned> {
    #[serde(default, bound = "")]
    pub _embedded: Option<T>,
    #[serde(default)]
    pub _links: HalLinks,
    #[serde(default)]
    pub page: PageStats,
}

// ── Studies ───────────────────────────────────────────────────────────────

/// `_embedded.studies` — list of [`RestStudy`].
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedRestStudies {
    #[serde(default)]
    pub studies: Vec<RestStudy>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestStudy {
    #[serde(default)]
    pub accession_id: String,
    #[serde(default)]
    pub initial_sample_size: String,
    #[serde(default)]
    pub replication_sample_size: String,
    #[serde(default)]
    pub gxe: bool,
    #[serde(default)]
    pub gxg: bool,
    #[serde(default)]
    pub snp_count: u64,
    #[serde(default)]
    pub qualifier: Option<String>,
    #[serde(default)]
    pub imputed: bool,
    #[serde(default)]
    pub pooled: bool,
    #[serde(default)]
    pub full_pvalue_set: bool,
    #[serde(default)]
    pub study_design_comment: Option<String>,
    #[serde(default)]
    pub publication_info: Option<PublicationInfo>,
    #[serde(default)]
    pub disease_trait: Option<DiseaseTrait>,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    #[serde(default)]
    pub genotyping_technologies: Vec<GenotypingTech>,
    #[serde(default)]
    pub ancestries: Vec<Ancestry>,
    #[serde(default)]
    pub _links: HalLinks,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationInfo {
    #[serde(default)]
    pub pubmed_id: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub publication: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<PublicationAuthor>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PublicationAuthor {
    #[serde(default)]
    pub fullname: Option<String>,
    #[serde(default)]
    pub orcid: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DiseaseTrait {
    #[serde(default, rename = "trait")]
    pub trait_name: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Platform {
    #[serde(default)]
    pub manufacturer: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenotypingTech {
    #[serde(default)]
    pub genotyping_technology: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ancestry {
    #[serde(default)]
    #[serde(rename = "type")]
    pub ancestry_type: Option<String>,
    #[serde(default)]
    pub number_of_individuals: u64,
    #[serde(default)]
    pub country_of_recruitment: Vec<Country>,
    #[serde(default)]
    pub ancestral_groups: Vec<AncestralGroup>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Country {
    #[serde(default)]
    pub major_area: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub country_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AncestralGroup {
    #[serde(default)]
    pub ancestral_group: Option<String>,
}

// ── Associations ──────────────────────────────────────────────────────────

/// `_embedded.associations` — list of [`RestAssociation`].
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedRestAssociations {
    #[serde(default)]
    pub associations: Vec<RestAssociation>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestAssociation {
    #[serde(default)]
    pub risk_frequency: Option<String>,
    #[serde(default)]
    pub pvalue_description: Option<String>,
    #[serde(default)]
    pub pvalue_mantissa: Option<u32>,
    #[serde(default)]
    pub pvalue_exponent: Option<i32>,
    #[serde(default)]
    pub pvalue: Option<f64>,
    #[serde(default)]
    pub multi_snp_haplotype: bool,
    #[serde(default)]
    pub snp_interaction: bool,
    #[serde(default)]
    pub snp_type: Option<String>,
    #[serde(default)]
    pub standard_error: Option<f64>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub or_per_copy_num: Option<f64>,
    #[serde(default)]
    pub beta_num: Option<f64>,
    #[serde(default)]
    pub beta_unit: Option<String>,
    #[serde(default)]
    pub beta_direction: Option<String>,
    #[serde(default)]
    pub loci: Vec<Locus>,
    #[serde(default)]
    pub _links: HalLinks,
}

impl RestAssociation {
    /// Best-effort rsID extraction from loci → strongestRiskAlleles → snp link.
    pub fn rsid(&self) -> Option<&str> {
        self.loci
            .first()?
            .strongest_risk_alleles
            .first()?
            ._links
            .snp
            .as_ref()?
            .href
            .rsplit('/')
            .next()
    }

    /// All author-reported gene names across loci.
    pub fn genes(&self) -> Vec<&str> {
        self.loci
            .iter()
            .flat_map(|l| {
                l.author_reported_genes
                    .iter()
                    .filter_map(|g| g.gene_name.as_deref())
            })
            .collect()
    }

    /// All Entrez gene IDs across loci.
    pub fn entrez_ids(&self) -> Vec<&str> {
        self.loci
            .iter()
            .flat_map(|l| {
                l.author_reported_genes
                    .iter()
                    .flat_map(|g| {
                        g.entrez_gene_ids
                            .iter()
                            .filter_map(|id| id.entrez_gene_id.as_deref())
                    })
            })
            .collect()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Locus {
    #[serde(default)]
    pub haplotype_snp_count: Option<u32>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub strongest_risk_alleles: Vec<RiskAllele>,
    #[serde(default)]
    pub author_reported_genes: Vec<ReportedGene>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskAllele {
    #[serde(default)]
    pub risk_allele_name: Option<String>,
    #[serde(default)]
    pub risk_frequency: Option<String>,
    #[serde(default)]
    pub genome_wide: Option<bool>,
    #[serde(default, rename = "_links")]
    pub _links: HalLinks,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedGene {
    #[serde(default)]
    pub gene_name: Option<String>,
    #[serde(default)]
    pub entrez_gene_ids: Vec<GeneId>,
    #[serde(default)]
    pub ensembl_gene_ids: Vec<GeneId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneId {
    // `entrezGeneId` or `ensemblGeneId` depending on context
    #[serde(default)]
    pub entrez_gene_id: Option<String>,
    #[serde(default)]
    pub ensembl_gene_id: Option<String>,
}

// ── EFO Traits ─────────────────────────────────────────────────────────────

/// `_embedded.efoTraits` — list of [`EfoTrait`].
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedEfoTraits {
    #[serde(default, rename = "efoTraits")]
    pub efo_traits: Vec<EfoTrait>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EfoTrait {
    #[serde(default, rename = "trait")]
    pub trait_name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub short_form: String,
}

// ── SNPs ───────────────────────────────────────────────────────────────────

/// `_embedded.singleNucleotidePolymorphisms`.
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedSnps {
    #[serde(default, rename = "singleNucleotidePolymorphisms")]
    pub single_nucleotide_polymorphisms: Vec<Snp>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snp {
    #[serde(default)]
    pub rs_id: String,
    #[serde(default)]
    pub merged: u32,
    #[serde(default)]
    pub functional_class: Option<String>,
    #[serde(default)]
    pub last_update_date: Option<String>,
    #[serde(default)]
    pub locations: Vec<SnpLocation>,
    #[serde(default)]
    pub genomic_contexts: Vec<GenomicContext>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnpLocation {
    #[serde(default)]
    pub chromosome_name: String,
    #[serde(default)]
    pub chromosome_position: u64,
    #[serde(default)]
    pub region: Option<SnpRegion>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SnpRegion {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenomicContext {
    #[serde(default)]
    pub is_intergenic: bool,
    #[serde(default)]
    pub is_upstream: bool,
    #[serde(default)]
    pub is_downstream: bool,
    #[serde(default)]
    pub distance: i64,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub is_closest_gene: bool,
    #[serde(default)]
    pub gene: Option<ContextGene>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextGene {
    #[serde(default)]
    pub gene_name: Option<String>,
    #[serde(default)]
    pub entrez_gene_ids: Vec<GeneId>,
    #[serde(default)]
    pub ensembl_gene_ids: Vec<GeneId>,
}

impl Snp {
    /// Primary chromosomal location (first entry).
    pub fn primary_location(&self) -> Option<&SnpLocation> {
        self.locations.first()
    }

    /// All nearby gene names from genomic contexts.
    pub fn nearby_genes(&self) -> Vec<&str> {
        self.genomic_contexts
            .iter()
            .filter_map(|gc| gc.gene.as_ref()?.gene_name.as_deref())
            .collect()
    }
}

// ── Unpublished Studies ────────────────────────────────────────────────────

/// `_embedded.unpublishedStudies`.
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedUnpublishedStudies {
    #[serde(default, rename = "unpublishedStudies")]
    pub unpublished_studies: Vec<UnpublishedStudy>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnpublishedStudy {
    #[serde(default)]
    pub study_accession: String,
    #[serde(default)]
    pub study_tag: String,
    #[serde(default)]
    pub study_description: String,
    #[serde(default, rename = "trait")]
    pub trait_name: String,
    #[serde(default)]
    pub efo_trait: Option<String>,
    #[serde(default)]
    pub genotyping_technology: Option<String>,
    #[serde(default)]
    pub imputation: bool,
    #[serde(default)]
    pub sample_description: Option<String>,
    #[serde(default)]
    pub cohort: Option<String>,
    #[serde(default)]
    pub variant_count: u64,
    #[serde(default)]
    pub _links: HalLinks,
}

// ── Query enums ─────────────────────────────────────────────────────────────

/// Selects a `/studies/search/findBy*` endpoint.
#[derive(Debug, Clone)]
pub enum StudyQuery {
    Accession(String),
    DiseaseTrait(String),
    EfoTrait(String),
    EfoUri(String),
    Pmid(String),
    UserRequested(bool),
    FullPvalueSet(bool),
}

/// Selects an `/associations/search/findBy*` endpoint.
#[derive(Debug, Clone)]
pub enum AssociationQueryKind {
    RsId(String),
    StudyAccession(String),
    Pmid(String),
    EfoTrait(String),
    RsIdAndAccession { rs_id: String, accession: String },
}

/// Selects an `/efoTraits/search/findBy*` endpoint.
#[derive(Debug, Clone)]
pub enum EfoQuery {
    ShortForm(String),
    Uri(String),
    Trait(String),
    Pmid(String),
}

/// Selects a `/singleNucleotidePolymorphisms/search/findBy*` endpoint.
#[derive(Debug, Clone)]
pub enum SnpQuery {
    RsId(String),
    Gene(String),
    ChromBpRange {
        chrom: String,
        bp_start: u64,
        bp_end: u64,
    },
    DiseaseTrait(String),
    EfoTrait(String),
    Pmid(String),
}

/// Filter for `/unpublished-studies/search/filter`.
#[derive(Debug, Default, Clone)]
pub struct UnpublishedFilter {
    pub first_author: Option<String>,
    pub accession: Option<String>,
    pub title: Option<String>,
    pub trait_: Option<String>,
}

// ── Client methods (REST API) ──────────────────────────────────────────────

impl GwasCatalogClient {
    async fn rest_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        pairs: Vec<(&str, String)>,
    ) -> Result<T> {
        self.get(self.rest_base(), path, &pairs).await
    }

    /// `GET /studies` — list all curated studies (paginated).
    pub async fn rest_studies(
        &self,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestStudies>> {
        self.rest_get("/studies", page_size_pairs(page, size)).await
    }

    /// `GET /studies/search/findBy*` — search studies by various criteria.
    pub async fn rest_find_studies(
        &self,
        query: &StudyQuery,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestStudies>> {
        let (path, extra) = match query {
            StudyQuery::Accession(id) => (
                "/studies/search/findByAccessionId",
                vec![("accessionId", id.clone())],
            ),
            StudyQuery::DiseaseTrait(t) => (
                "/studies/search/findByDiseaseTrait",
                vec![("diseaseTrait", t.clone())],
            ),
            StudyQuery::EfoTrait(t) => (
                "/studies/search/findByEfoTrait",
                vec![("efoTrait", t.clone())],
            ),
            StudyQuery::EfoUri(uri) => {
                ("/studies/search/findByEfoUri", vec![("uri", uri.clone())])
            }
            StudyQuery::Pmid(pmid) => (
                "/studies/search/findByPublicationIdPubmedId",
                vec![("pubmedId", pmid.clone())],
            ),
            StudyQuery::UserRequested(v) => (
                "/studies/search/findByUserRequested",
                vec![("userRequested", v.to_string())],
            ),
            StudyQuery::FullPvalueSet(v) => (
                "/studies/search/findByFullPvalueSet",
                vec![("fullPvalueSet", v.to_string())],
            ),
        };
        let mut pairs = page_size_pairs(page, size);
        pairs.extend(extra);
        self.rest_get(path, pairs).await
    }

    /// `GET /studies/{accessionId}` — single study.
    pub async fn rest_get_study(&self, accession_id: &str) -> Result<RestStudy> {
        self.rest_get(&format!("/studies/{accession_id}"), vec![])
            .await
    }

    /// `GET /studies/{accessionId}/associations` — associations for a study.
    pub async fn rest_study_associations(
        &self,
        accession_id: &str,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestAssociations>> {
        self.rest_get(
            &format!("/studies/{accession_id}/associations"),
            page_size_pairs(page, size),
        )
        .await
    }

    /// `GET /associations/search/findBy*` — search associations.
    pub async fn rest_find_associations(
        &self,
        query: &AssociationQueryKind,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestAssociations>> {
        let (path, extra) = match query {
            AssociationQueryKind::RsId(rs) => {
                ("/associations/search/findByRsId", vec![("rsId", rs.clone())])
            }
            AssociationQueryKind::StudyAccession(acc) => (
                "/associations/search/findByStudyAccessionId",
                vec![("accessionId", acc.clone())],
            ),
            AssociationQueryKind::Pmid(pm) => (
                "/associations/search/findByPubmedId",
                vec![("pubmedId", pm.clone())],
            ),
            AssociationQueryKind::EfoTrait(t) => (
                "/associations/search/findByEfoTrait",
                vec![("efoTrait", t.clone())],
            ),
            AssociationQueryKind::RsIdAndAccession { rs_id, accession } => (
                "/associations/search/findByRsIdAndAccessionId",
                vec![("rsId", rs_id.clone()), ("accessionId", accession.clone())],
            ),
        };
        let mut pairs = page_size_pairs(page, size);
        pairs.extend(extra);
        self.rest_get(path, pairs).await
    }

    /// `GET /associations/{id}` — single association by numeric ID.
    pub async fn rest_get_association(&self, id: u64) -> Result<RestAssociation> {
        self.rest_get(&format!("/associations/{id}"), vec![])
            .await
    }

    /// `GET /efoTraits/search/findBy*` — search EFO traits.
    pub async fn rest_find_efo_traits(
        &self,
        query: &EfoQuery,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedEfoTraits>> {
        let (path, extra) = match query {
            EfoQuery::ShortForm(sf) => (
                "/efoTraits/search/findByShortForm",
                vec![("shortForm", sf.clone())],
            ),
            EfoQuery::Uri(uri) => (
                "/efoTraits/search/findByEfoUri",
                vec![("uri", uri.clone())],
            ),
            EfoQuery::Trait(t) => (
                "/efoTraits/search/findByEfoTrait",
                vec![("trait", t.clone())],
            ),
            EfoQuery::Pmid(pm) => (
                "/efoTraits/search/findByPubmedId",
                vec![("pubmedId", pm.clone())],
            ),
        };
        let mut pairs = page_size_pairs(page, size);
        pairs.extend(extra);
        self.rest_get(path, pairs).await
    }

    /// `GET /efoTraits/{shortForm}` — single EFO trait.
    pub async fn rest_get_efo_trait(&self, short_form: &str) -> Result<EfoTrait> {
        self.rest_get(&format!("/efoTraits/{short_form}"), vec![])
            .await
    }

    /// `GET /efoTraits/{shortForm}/associations`
    pub async fn rest_efo_trait_associations(
        &self,
        short_form: &str,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestAssociations>> {
        self.rest_get(
            &format!("/efoTraits/{short_form}/associations"),
            page_size_pairs(page, size),
        )
        .await
    }

    /// `GET /efoTraits/{shortForm}/studies`
    pub async fn rest_efo_trait_studies(
        &self,
        short_form: &str,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestStudies>> {
        self.rest_get(
            &format!("/efoTraits/{short_form}/studies"),
            page_size_pairs(page, size),
        )
        .await
    }

    /// `GET /singleNucleotidePolymorphisms/search/findBy*` — search SNPs.
    pub async fn rest_find_snps(
        &self,
        query: &SnpQuery,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedSnps>> {
        let (path, extra) = match query {
            SnpQuery::RsId(rs) => (
                "/singleNucleotidePolymorphisms/search/findByRsId",
                vec![("rsId", rs.clone())],
            ),
            SnpQuery::Gene(g) => (
                "/singleNucleotidePolymorphisms/search/findByGene",
                vec![("geneName", g.clone())],
            ),
            SnpQuery::ChromBpRange {
                chrom,
                bp_start,
                bp_end,
            } => (
                "/singleNucleotidePolymorphisms/search/findByChromBpLocationRange",
                vec![
                    ("chrom", chrom.clone()),
                    ("bpStart", bp_start.to_string()),
                    ("bpEnd", bp_end.to_string()),
                ],
            ),
            SnpQuery::DiseaseTrait(t) => (
                "/singleNucleotidePolymorphisms/search/findByDiseaseTrait",
                vec![("diseaseTrait", t.clone())],
            ),
            SnpQuery::EfoTrait(t) => (
                "/singleNucleotidePolymorphisms/search/findByEfoTrait",
                vec![("efoTrait", t.clone())],
            ),
            SnpQuery::Pmid(pm) => (
                "/singleNucleotidePolymorphisms/search/findByPubmedId",
                vec![("pubmedId", pm.clone())],
            ),
        };
        let mut pairs = page_size_pairs(page, size);
        pairs.extend(extra);
        self.rest_get(path, pairs).await
    }

    /// `GET /singleNucleotidePolymorphisms/{rsId}` — single SNP.
    pub async fn rest_get_snp(&self, rs_id: &str) -> Result<Snp> {
        self.rest_get(&format!("/singleNucleotidePolymorphisms/{rs_id}"), vec![])
            .await
    }

    /// `GET /singleNucleotidePolymorphisms/{rsId}/associations`
    pub async fn rest_snp_associations(
        &self,
        rs_id: &str,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedRestAssociations>> {
        self.rest_get(
            &format!("/singleNucleotidePolymorphisms/{rs_id}/associations"),
            page_size_pairs(page, size),
        )
        .await
    }

    /// `GET /unpublished-studies/search/filter` — search unpublished submissions.
    pub async fn rest_unpublished_studies(
        &self,
        filter: &UnpublishedFilter,
        page: Option<u32>,
        size: Option<u32>,
    ) -> Result<RestPage<EmbeddedUnpublishedStudies>> {
        let mut pairs = page_size_pairs(page, size);
        if let Some(v) = &filter.first_author {
            pairs.push(("firstAuthor", v.clone()));
        }
        if let Some(v) = &filter.accession {
            pairs.push(("accession", v.clone()));
        }
        if let Some(v) = &filter.title {
            pairs.push(("title", v.clone()));
        }
        if let Some(v) = &filter.trait_ {
            pairs.push(("trait", v.clone()));
        }
        self.rest_get("/unpublished-studies/search/filter", pairs).await
    }
}

/// Build `page`/`size` query pairs for Spring Data REST pagination.
fn page_size_pairs(page: Option<u32>, size: Option<u32>) -> Vec<(&'static str, String)> {
    let mut pairs = Vec::new();
    if let Some(p) = page {
        pairs.push(("page", p.to_string()));
    }
    if let Some(s) = size {
        pairs.push(("size", s.to_string()));
    }
    pairs
}
