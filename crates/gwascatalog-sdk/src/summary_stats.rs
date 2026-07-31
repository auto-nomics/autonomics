//! GWAS Catalog Summary Statistics API — types and client methods.
//!
//! <https://www.ebi.ac.uk/gwas/summary-statistics/docs/>
//!
//! All endpoints are read-only (`GET`). Responses use HAL format with
//! `_links` (pagination: `first`, `next`) and `_embedded` (data).

use serde::Deserialize;

use crate::client::GwasCatalogClient;
use crate::error::{GwasCatalogError, Result};

// ── Data types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HalLink {
    pub href: String,
}

/// HAL `_links` object. Fields appear depending on resource type.
#[derive(Debug, Default, Deserialize)]
pub struct HalLinks {
    #[serde(default, rename = "self")]
    pub self_: Option<HalLink>,
    #[serde(default, rename = "first")]
    pub first: Option<HalLink>,
    #[serde(default, rename = "next")]
    pub next: Option<HalLink>,
    #[serde(default, rename = "associations")]
    pub associations: Option<HalLink>,
    #[serde(default, rename = "studies")]
    pub studies: Option<HalLink>,
    #[serde(default, rename = "traits")]
    pub traits: Option<HalLink>,
    #[serde(default, rename = "chromosomes")]
    pub chromosomes: Option<HalLink>,
    #[serde(
        default,
        rename = "trait",
        deserialize_with = "single_or_vec::deserialize"
    )]
    pub trait_: Option<Vec<HalLink>>,
    #[serde(default, rename = "variant")]
    pub variant: Option<HalLink>,
    #[serde(default, rename = "study")]
    pub study: Option<HalLink>,
    #[serde(default, rename = "ols")]
    pub ols: Option<HalLink>,
    #[serde(default, rename = "gwas_catalog")]
    pub gwas_catalog: Option<HalLink>,
    #[serde(default, rename = "snp")]
    pub snp: Option<HalLink>,
}

/// A single variant–study association record.
///
/// Default values are harmonised; use `reveal=raw` or `reveal=all` to access
/// original data (harmonised fields get `hm_` prefix with `reveal=all`).
#[derive(Debug, Deserialize)]
pub struct Association {
    pub variant_id: String,
    pub chromosome: u32,
    pub base_pair_location: u64,
    pub study_accession: String,
    #[serde(rename = "trait")]
    pub trait_: Vec<String>,
    #[serde(with = "p_value_serde")]
    pub p_value: f64,
    pub code: Option<u32>,
    pub effect_allele: Option<String>,
    pub other_allele: Option<String>,
    pub effect_allele_frequency: Option<f64>,
    pub odds_ratio: Option<f64>,
    pub ci_lower: Option<f64>,
    pub ci_upper: Option<f64>,
    pub beta: Option<f64>,
    pub se: Option<f64>,
    #[serde(default)]
    pub _links: HalLinks,
}

mod single_or_vec {
    use super::HalLink;
    use serde::de::{self, Deserialize, Deserializer, IntoDeserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<HalLink>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<serde_json::Value>::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(serde_json::Value::Array(arr)) => {
                let de = arr.into_deserializer();
                Vec::<HalLink>::deserialize(de)
                    .map(Some)
                    .map_err(de::Error::custom)
            }
            Some(other) => {
                let de = other.into_deserializer();
                let link = HalLink::deserialize(de).map_err(de::Error::custom)?;
                Ok(Some(vec![link]))
            }
        }
    }
}

mod p_value_serde {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        match v {
            serde_json::Value::String(s) => s
                .replace(' ', "")
                .parse::<f64>()
                .map_err(serde::de::Error::custom),
            serde_json::Value::Number(n) => n
                .as_f64()
                .ok_or_else(|| serde::de::Error::custom("invalid p-value number")),
            _ => Err(serde::de::Error::custom(
                "expected string or number for p_value",
            )),
        }
    }
}

/// Chromosome resource.
#[derive(Debug, Deserialize)]
pub struct Chromosome {
    pub chromosome: String,
    #[serde(default)]
    pub _links: HalLinks,
}

/// EFO trait resource.
#[derive(Debug, Deserialize)]
pub struct Trait {
    #[serde(rename = "trait")]
    pub trait_: String,
    #[serde(default)]
    pub _links: HalLinks,
}

/// Study resource.
#[derive(Debug, Deserialize)]
pub struct Study {
    pub study_accession: String,
    #[serde(default)]
    pub _links: HalLinks,
}

/// `_embedded` wrapper for association lists ( keyed by index string ).
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedAssociations {
    #[serde(default)]
    pub associations: std::collections::HashMap<String, Association>,
}

/// `_embedded` wrapper for chromosome lists.
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedChromosomes {
    #[serde(default)]
    pub chromosomes: Vec<Chromosome>,
}

/// `_embedded` wrapper for trait lists.
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedTraits {
    #[serde(default, rename = "trait")]
    pub traits: Vec<Trait>,
}

/// `_embedded` wrapper for study lists (returned by `/studies`).
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedStudies {
    #[serde(default)]
    pub studies: Vec<Vec<Study>>,
}

/// `_embedded` wrapper for trait-specific study lists (returned by `/traits/{trait}/studies`).
#[derive(Debug, Default, Deserialize)]
pub struct EmbeddedTraitStudies {
    #[serde(default)]
    pub studies: Vec<Study>,
}

/// Generic HAL paginated response.
///
/// Use `_links.next` to fetch the next page; the `start` offset in the next link
/// may not equal `previous_start + size` when filtering by p-value or base-pair range.
#[derive(Debug, Deserialize)]
#[serde(bound = "")]
pub struct PaginatedResponse<T: serde::de::DeserializeOwned> {
    #[serde(default, bound = "")]
    pub _embedded: Option<T>,
    #[serde(default)]
    pub _links: HalLinks,
}

// ── Query builders ────────────────────────────────────────────────────────

/// Query parameters for association endpoints.
#[derive(Debug, Default, Clone)]
pub struct AssociationQuery {
    pub start: Option<usize>,
    pub size: Option<usize>,
    pub reveal: Option<RevealMode>,
    pub p_lower: Option<f64>,
    pub p_upper: Option<f64>,
    pub study_accession: Option<String>,
}

/// Query parameters for chromosome-specific association endpoints.
#[derive(Debug, Default, Clone)]
pub struct ChromosomeAssociationQuery {
    pub start: Option<usize>,
    pub size: Option<usize>,
    pub reveal: Option<RevealMode>,
    pub p_lower: Option<f64>,
    pub p_upper: Option<f64>,
    pub bp_lower: Option<u64>,
    pub bp_upper: Option<u64>,
    pub study_accession: Option<String>,
    pub trait_: Option<String>,
}

/// Controls what data `reveal` returns.
/// - `Raw` — original/unharmonised values only
/// - `All` — both harmonised (default) and raw; raw fields get `hm_` prefix
#[derive(Debug, Clone, Copy)]
pub enum RevealMode {
    Raw,
    All,
}

impl std::fmt::Display for RevealMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevealMode::Raw => write!(f, "raw"),
            RevealMode::All => write!(f, "all"),
        }
    }
}

/// Basic pagination query (`start` + `size`).
#[derive(Debug, Default, Clone)]
pub struct PaginationQuery {
    pub start: Option<usize>,
    pub size: Option<usize>,
}

// ── Query → URL pairs ──────────────────────────────────────────────────────

trait QueryParams: Sync {
    fn query_pairs(&self) -> Vec<(&'static str, String)>;
}

impl QueryParams for () {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

impl QueryParams for PaginationQuery {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        if let Some(v) = self.start {
            p.push(("start", v.to_string()));
        }
        if let Some(v) = self.size {
            p.push(("size", v.to_string()));
        }
        p
    }
}

impl QueryParams for AssociationQuery {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        if let Some(v) = self.start {
            p.push(("start", v.to_string()));
        }
        if let Some(v) = self.size {
            p.push(("size", v.to_string()));
        }
        if let Some(r) = &self.reveal {
            p.push(("reveal", r.to_string()));
        }
        if let Some(v) = self.p_lower {
            p.push(("p_lower", v.to_string()));
        }
        if let Some(v) = self.p_upper {
            p.push(("p_upper", v.to_string()));
        }
        if let Some(v) = &self.study_accession {
            p.push(("study_accession", v.clone()));
        }
        p
    }
}

impl QueryParams for ChromosomeAssociationQuery {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        if let Some(v) = self.start {
            p.push(("start", v.to_string()));
        }
        if let Some(v) = self.size {
            p.push(("size", v.to_string()));
        }
        if let Some(r) = &self.reveal {
            p.push(("reveal", r.to_string()));
        }
        if let Some(v) = self.p_lower {
            p.push(("p_lower", v.to_string()));
        }
        if let Some(v) = self.p_upper {
            p.push(("p_upper", v.to_string()));
        }
        if let Some(v) = self.bp_lower {
            p.push(("bp_lower", v.to_string()));
        }
        if let Some(v) = self.bp_upper {
            p.push(("bp_upper", v.to_string()));
        }
        if let Some(v) = &self.study_accession {
            p.push(("study_accession", v.clone()));
        }
        if let Some(v) = &self.trait_ {
            p.push(("trait", v.clone()));
        }
        p
    }
}

// ── Client methods (Summary Statistics API) ───────────────────────────────

impl GwasCatalogClient {
    async fn ss_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &dyn QueryParams,
    ) -> Result<T> {
        self.get(self.ss_base(), path, &query.query_pairs()).await
    }

    /// `GET /associations` — list all available associations.
    pub async fn list_associations(
        &self,
        query: &AssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get("/associations", query).await
    }

    /// `GET /associations/{variant_id}` — associations for a variant (rsid).
    pub async fn get_variant_associations(
        &self,
        variant_id: &str,
        query: &AssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get(&format!("/associations/{variant_id}"), query)
            .await
    }

    /// `GET /chromosomes` — list all chromosome resources.
    pub async fn list_chromosomes(
        &self,
        query: &PaginationQuery,
    ) -> Result<PaginatedResponse<EmbeddedChromosomes>> {
        self.ss_get("/chromosomes", query).await
    }

    /// `GET /chromosomes/{chromosome}` — specific chromosome resource.
    pub async fn get_chromosome(&self, chromosome: &str) -> Result<Chromosome> {
        self.ss_get(&format!("/chromosomes/{chromosome}"), &()).await
    }

    /// `GET /chromosomes/{chromosome}/associations` — associations on a chromosome.
    pub async fn list_chromosome_associations(
        &self,
        chromosome: &str,
        query: &ChromosomeAssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get(
            &format!("/chromosomes/{chromosome}/associations"),
            query,
        )
        .await
    }

    /// `GET /chromosomes/{chromosome}/associations/{variant_id}`
    pub async fn get_variant_on_chromosome(
        &self,
        chromosome: &str,
        variant_id: &str,
        query: &AssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get(
            &format!("/chromosomes/{chromosome}/associations/{variant_id}"),
            query,
        )
        .await
    }

    /// `GET /traits` — list all EFO trait resources.
    pub async fn list_traits(
        &self,
        query: &PaginationQuery,
    ) -> Result<PaginatedResponse<EmbeddedTraits>> {
        self.ss_get("/traits", query).await
    }

    /// `GET /traits/{trait}` — specific trait resource.
    pub async fn get_trait(&self, trait_id: &str) -> Result<Trait> {
        self.ss_get(&format!("/traits/{trait_id}"), &()).await
    }

    /// `GET /traits/{trait}/associations`
    pub async fn list_trait_associations(
        &self,
        trait_id: &str,
        query: &AssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get(&format!("/traits/{trait_id}/associations"), query)
            .await
    }

    /// `GET /traits/{trait}/studies`
    pub async fn list_trait_studies(
        &self,
        trait_id: &str,
        query: &PaginationQuery,
    ) -> Result<PaginatedResponse<EmbeddedTraitStudies>> {
        self.ss_get(&format!("/traits/{trait_id}/studies"), query)
            .await
    }

    /// `GET /studies` — list all study resources.
    pub async fn list_studies(
        &self,
        query: &PaginationQuery,
    ) -> Result<PaginatedResponse<EmbeddedStudies>> {
        self.ss_get("/studies", query).await
    }

    /// `GET /studies/{study_accession}` — specific study resource.
    pub async fn get_study(&self, study_accession: &str) -> Result<Study> {
        self.ss_get(&format!("/studies/{study_accession}"), &())
            .await
    }

    /// `GET /studies/{study_accession}/associations`
    pub async fn list_study_associations(
        &self,
        study_accession: &str,
        query: &AssociationQuery,
    ) -> Result<PaginatedResponse<EmbeddedAssociations>> {
        self.ss_get(
            &format!("/studies/{study_accession}/associations"),
            query,
        )
        .await
    }
}
