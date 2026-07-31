//! GWAS Catalog Solr Search API — types and client methods.
//!
//! <https://www.ebi.ac.uk/gwas/api/search>
//!
//! Apache Solr-backed full-text search across all resource types
//! (study, variant, trait, gene, publication) with faceted results
//! and numeric/range/array filters.

use serde::Deserialize;

use crate::client::GwasCatalogClient;
use crate::error::Result;

// ── Response types ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseHeader {
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub q_time: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolrResponse {
    #[serde(default)]
    pub num_found: u64,
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub docs: Vec<SearchDoc>,
}

/// Heterogeneous Solr document — fields vary by `resourcename`.
///
/// All fields are optional; Solr returns only indexed fields per document type.
#[derive(Debug, Default, Deserialize)]
pub struct SearchDoc {
    #[serde(default)]
    pub resourcename: Option<String>,
    // Study fields
    #[serde(default)]
    pub accession_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub association_count: Option<u64>,
    #[serde(default)]
    pub full_pvalue_set: Option<bool>,
    #[serde(default)]
    pub published: Option<String>,
    // Variant fields
    #[serde(default, rename = "rsID")]
    pub rs_id: Option<String>,
    #[serde(default)]
    pub chromosome_name: Option<String>,
    #[serde(default)]
    pub chromosome_position: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub consequence: Option<String>,
    #[serde(default)]
    pub mapped_genes: Vec<String>,
    #[serde(default)]
    pub study_count: Option<u64>,
    // Trait fields
    #[serde(default)]
    pub mapped_trait: Option<String>,
    #[serde(default)]
    pub short_form: Option<String>,
    #[serde(default)]
    pub reported_trait_s: Option<String>,
    // Gene fields
    #[serde(default)]
    pub ensembl_id: Option<String>,
    #[serde(default)]
    pub entrez_id: Option<String>,
    #[serde(default)]
    pub biotype: Option<String>,
    #[serde(default)]
    pub cytobands: Option<String>,
    // Publication fields
    #[serde(default)]
    pub pmid: Option<String>,
    #[serde(default)]
    pub journal: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub author: Vec<String>,
}

impl SearchDoc {
    /// Human-readable label for this document, based on resource type.
    pub fn label(&self) -> String {
        match self.resourcename.as_deref() {
            Some("study") => self
                .title
                .clone()
                .unwrap_or_else(|| self.accession_id.clone().unwrap_or_default()),
            Some("variant") => self.rs_id.clone().unwrap_or_default(),
            Some("trait") => self.mapped_trait.clone().unwrap_or_default(),
            Some("gene") => self.title.clone().unwrap_or_default(),
            Some("publication") => self.title.clone().unwrap_or_default(),
            _ => self
                .accession_id
                .clone()
                .or_else(|| self.rs_id.clone())
                .or_else(|| self.mapped_trait.clone())
                .unwrap_or_default(),
        }
    }
}

/// Solr facet counts. `facet_fields` maps field name → alternating
/// (value, count) pairs.
#[derive(Debug, Default, Deserialize)]
pub struct FacetCounts {
    #[serde(default)]
    pub facet_fields: std::collections::HashMap<String, Vec<serde_json::Value>>,
}

/// Top-level Solr search response.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    #[serde(default)]
    pub response_header: ResponseHeader,
    #[serde(default)]
    pub response: SolrResponse,
    #[serde(default)]
    pub facet_counts: Option<FacetCounts>,
}

// ── Query builder ───────────────────────────────────────────────────────────

/// Filter parameters for the Solr Search API.
#[derive(Debug, Default, Clone)]
pub struct SearchFilter {
    /// Solr query string (required). `*:*` matches all.
    pub q: String,
    pub max: Option<u32>,
    pub start: Option<u32>,
    pub pval_filter: Option<String>,
    pub or_filter: Option<String>,
    pub beta_filter: Option<String>,
    pub date_filter: Option<String>,
    /// `"chrom:start-end"` — e.g. `"1:1000000-2000000"`.
    pub genomic_filter: Option<String>,
    /// EFO URI(s), e.g. `["EFO_0000400"]`.
    pub trait_filter: Vec<String>,
    pub genotyping_tech_filter: Vec<String>,
    pub ancestry_filter: Vec<String>,
}

impl SearchFilter {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        p.push(("q", self.q.clone()));
        if let Some(v) = self.max {
            p.push(("max", v.to_string()));
        }
        if let Some(v) = self.start {
            p.push(("start", v.to_string()));
        }
        if let Some(v) = &self.pval_filter {
            p.push(("pvalfilter", v.clone()));
        }
        if let Some(v) = &self.or_filter {
            p.push(("orfilter", v.clone()));
        }
        if let Some(v) = &self.beta_filter {
            p.push(("betafilter", v.clone()));
        }
        if let Some(v) = &self.date_filter {
            p.push(("datefilter", v.clone()));
        }
        if let Some(v) = &self.genomic_filter {
            p.push(("genomicfilter", v.clone()));
        }
        p
    }

    /// Array-style filter params (`traitfilter[]`, etc.) — these must be
    /// appended to the URL separately as repeated query keys.
    fn array_pairs(&self) -> Vec<(String, String)> {
        let mut p = Vec::new();
        for t in &self.trait_filter {
            p.push(("traitfilter[]".to_string(), t.clone()));
        }
        for t in &self.genotyping_tech_filter {
            p.push(("genotypingTechfilter[]".to_string(), t.clone()));
        }
        for t in &self.ancestry_filter {
            p.push(("ancestryfilter[]".to_string(), t.clone()));
        }
        p
    }
}

// ── Client method ───────────────────────────────────────────────────────────

impl GwasCatalogClient {
    /// `GET` the Solr Search API with the given [`SearchFilter`].
    pub async fn search(&self, filter: &SearchFilter) -> Result<SearchResponse> {
        let static_pairs = filter.query_pairs();
        let array_pairs = filter.array_pairs();

        // Build full URL — the `get` helper takes &[(&str, String)] but
        // array params have owned key strings, so we build manually.
        let mut url = format!("{}", self.search_base());
        let mut first = true;
        let add = |url: &mut String, first: &mut bool, k: &str, v: &str| {
            url.push(if *first { '?' } else { '&' });
            *first = false;
            url.push_str(k);
            url.push('=');
            url.push_str(v);
        };
        for (k, v) in &static_pairs {
            add(&mut url, &mut first, k, v);
        }
        for (k, v) in &array_pairs {
            add(&mut url, &mut first, k, v);
        }

        let resp = reqwest::get(&url).await?;
        let status = resp.status();
        if status.is_success() {
            resp.json().await.map_err(crate::error::GwasCatalogError::Http)
        } else {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let message = body["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            Err(crate::error::GwasCatalogError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}
