//! Serde request/response models for the OpenAlex REST API.
//!
//! OpenAlex entities are rich and frequently gain new fields, so every struct
//! uses `#[serde(default)]` liberally and keeps rarely-used nested objects as
//! `serde_json::Value` for forward compatibility.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Serde helpers
// ---------------------------------------------------------------------------

/// Deserialize `null` as an empty `Vec<T>`.
///
/// OpenAlex returns `"issn": null` (not `"issn": []`) for sources without an
/// ISSN.  Struct-level `#[serde(default)]` only covers *missing* fields, so we
/// need this deserializer for fields that are *present but null*.
pub(crate) fn null_to_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let opt: Option<Vec<T>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

// ===========================================================================
// Generic response envelope
// ===========================================================================

/// The `meta` object returned by every list endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub count: u64,
    #[serde(default)]
    pub db_response_time_ms: Option<f64>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
    #[serde(default)]
    pub per_page_raw: Option<String>,
    /// Cursor to pass to the next request for deep paging.
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// Cost of this API call in USD (present on paid endpoints).
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Random seed echoed back when using `sample` + `seed`.
    #[serde(default)]
    pub seed: Option<u64>,
}

/// One row of a `group_by` aggregation result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupByEntry {
    pub key: String,
    pub key_name: Option<String>,
    pub key_display_name: Option<String>,
    pub count: u64,
}

/// The generic list-response envelope.
///
/// `meta` carries counts and the next-page cursor; `results` is the array of
/// entity objects; `group_by` is populated only when the `group_by` query
/// parameter is supplied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub meta: Meta,
    #[serde(default = "Vec::new")]
    pub results: Vec<T>,
    #[serde(default = "Vec::new")]
    pub group_by: Vec<GroupByEntry>,
}

// ===========================================================================
// Work
// ===========================================================================

/// A scholarly document (article, book, dataset, thesis, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Work {
    pub id: String,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub publication_year: Option<u16>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub ids: WorkIds,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default, rename = "type")]
    pub type_: Option<String>,
    #[serde(default)]
    pub cited_by_count: u64,
    #[serde(default)]
    pub biblio: Option<Biblio>,
    #[serde(default)]
    pub is_retracted: bool,
    #[serde(default)]
    pub is_paratext: bool,
    #[serde(default)]
    pub open_access: OpenAccess,
    #[serde(default)]
    pub primary_location: Option<Location>,
    #[serde(default)]
    pub best_oa_location: Option<Location>,
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default)]
    pub authorships: Vec<Authorship>,
    #[serde(default)]
    pub abstract_inverted_index: Option<BTreeMap<String, Vec<u32>>>,
    #[serde(default)]
    pub concepts: Vec<ConceptScore>,
    #[serde(default)]
    pub topics: Vec<TopicAssignment>,
    #[serde(default)]
    pub keywords: Vec<KeywordAssignment>,
    #[serde(default)]
    pub mesh: Vec<MeshTerm>,
    #[serde(default)]
    pub referenced_works: Vec<String>,
    #[serde(default)]
    pub related_works: Vec<String>,
    #[serde(default)]
    pub cited_by_api_url: Option<String>,
    #[serde(default)]
    pub counts_by_year: Vec<CountByYear>,
    #[serde(default)]
    pub updated_date: Option<String>,
    #[serde(default)]
    pub created_date: Option<String>,
}

impl Work {
    /// Return the best available title (`display_name` falls back to `title`).
    ///
    /// OpenAlex sends both `display_name` and `title` (always identical);
    /// this method normalises the two fields into one.
    pub fn title_or_name(&self) -> Option<&str> {
        self.display_name
            .as_deref()
            .or(self.title.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// Reconstruct the plain-text abstract from the inverted index, if present.
    pub fn abstract_text(&self) -> Option<String> {
        let inv = self.abstract_inverted_index.as_ref()?;
        if inv.is_empty() {
            return None;
        }
        let mut pairs: Vec<(u32, &str)> = Vec::new();
        for (word, positions) in inv {
            for &pos in positions {
                pairs.push((pos, word));
            }
        }
        pairs.sort_by_key(|(pos, _)| *pos);
        let words: Vec<&str> = pairs.iter().map(|(_, w)| *w).collect();
        Some(words.join(" "))
    }
}

/// External identifiers for a [`Work`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkIds {
    pub openalex: Option<String>,
    pub doi: Option<String>,
    pub pmid: Option<String>,
    pub pmcid: Option<String>,
    pub mag: Option<String>,
}

/// Bibliographic metadata (volume, issue, pages).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Biblio {
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub first_page: Option<String>,
    pub last_page: Option<String>,
}

/// Open-access status of a [`Work`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAccess {
    pub is_oa: bool,
    pub oa_status: Option<String>,
    pub oa_url: Option<String>,
    pub any_repository_has_fulltext: bool,
}

/// A hosting location for a [`Work`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Location {
    pub is_oa: bool,
    pub landing_page_url: Option<String>,
    pub pdf_url: Option<String>,
    pub source: Option<DehydratedSource>,
    pub license: Option<String>,
    pub version: Option<String>,
}

/// A dehydrated (summary) source/venue object.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DehydratedSource {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub issn_l: Option<String>,
    pub issn: Option<Vec<String>>,
    pub host_organization: Option<String>,
    pub host_organization_name: Option<String>,
    pub type_: Option<String>,
}

/// An author and their institutional affiliations within a [`Work`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Authorship {
    pub author_position: Option<String>,
    pub author: DehydratedAuthor,
    pub institutions: Vec<DehydratedInstitution>,
    pub raw_affiliation_strings: Vec<String>,
}

/// A dehydrated author summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DehydratedAuthor {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub orcid: Option<String>,
}

/// A dehydrated institution summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DehydratedInstitution {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub ror: Option<String>,
    pub country_code: Option<String>,
    pub type_: Option<String>,
}

/// A concept score (legacy taxonomy).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConceptScore {
    pub id: String,
    pub wikidata: Option<String>,
    pub display_name: String,
    pub level: Option<u32>,
    pub score: Option<f64>,
}

/// A topic assignment for a work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopicAssignment {
    pub id: String,
    pub display_name: String,
    pub subfield: Option<DehydratedEntity>,
    pub field: Option<DehydratedEntity>,
    pub domain: Option<DehydratedEntity>,
    pub score: Option<f64>,
}

/// A generic dehydrated entity used by topics, subfields, fields, domains.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DehydratedEntity {
    pub id: String,
    pub display_name: String,
}

/// A keyword on a work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeywordAssignment {
    pub id: String,
    pub display_name: String,
    pub score: Option<f64>,
}

/// A MeSH term (PubMed works only).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshTerm {
    pub descriptor_ui: Option<String>,
    pub descriptor_name: Option<String>,
    pub qualifier_ui: Option<String>,
    pub qualifier_name: Option<String>,
    pub is_major_topic: Option<bool>,
}

/// Citation counts binned by year.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CountByYear {
    pub year: u16,
    pub cited_by_count: u64,
    pub works_count: Option<u64>,
}

// ===========================================================================
// Author
// ===========================================================================

/// A disambiguated researcher profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Author {
    pub id: String,
    pub display_name: String,
    pub display_name_alternatives: Vec<String>,
    pub orcid: Option<String>,
    pub works_count: u64,
    pub cited_by_count: u64,
    pub summary_stats: Option<AuthorSummaryStats>,
    pub last_known_institutions: Vec<DehydratedInstitution>,
    pub affiliations: Vec<AuthorAffiliation>,
    pub topics: Vec<TopicShare>,
    pub ids: Option<AuthorIds>,
    pub updated_date: Option<String>,
    pub created_date: Option<String>,
}

/// Summary statistics for an author.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthorSummaryStats {
    pub h_index: Option<u32>,
    pub i10_index: Option<u32>,
    pub twoyr_mean_citedness: Option<f64>,
    pub works_count: Option<u64>,
    pub cited_by_count: Option<u64>,
    pub oi: Option<f64>,
}

/// External IDs for an author.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthorIds {
    pub openalex: Option<String>,
    pub orcid: Option<String>,
    pub mag: Option<String>,
}

/// An institutional affiliation within an author record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthorAffiliation {
    pub institution: DehydratedInstitution,
    pub years: Vec<u16>,
    pub summary_stats: Option<AuthorSummaryStats>,
}

/// A topic with share in an author's output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopicShare {
    pub id: String,
    pub display_name: String,
    pub value: Option<f64>,
    pub count: Option<u64>,
    pub subfield: Option<DehydratedEntity>,
    pub field: Option<DehydratedEntity>,
    pub domain: Option<DehydratedEntity>,
}

// ===========================================================================
// Source (Journal / Repository)
// ===========================================================================

/// A journal, repository, or conference venue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub id: String,
    pub display_name: String,
    pub issn_l: Option<String>,
    #[serde(deserialize_with = "null_to_empty_vec")]
    pub issn: Vec<String>,
    pub host_organization: Option<String>,
    pub host_organization_name: Option<String>,
    pub publisher: Option<String>,
    pub works_count: u64,
    pub cited_by_count: u64,
    pub is_oa: bool,
    pub is_in_doaj: bool,
    pub homepage_url: Option<String>,
    pub type_: Option<String>,
    pub summary_stats: Option<SourceSummaryStats>,
    pub updated_date: Option<String>,
}

/// Summary stats for a source.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceSummaryStats {
    pub twoyr_mean_citedness: Option<f64>,
    pub h_index: Option<u32>,
    pub i10_index: Option<u32>,
    pub works_count: Option<u64>,
    pub cited_by_count: Option<u64>,
}

// ===========================================================================
// Institution
// ===========================================================================

/// A university or research organisation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Institution {
    pub id: String,
    pub display_name: String,
    pub ror: Option<String>,
    pub country_code: Option<String>,
    pub type_: Option<String>,
    pub lineage: Vec<String>,
    pub ancestor: Vec<DehydratedInstitution>,
    pub associated_institutions: Vec<AssociatedInstitution>,
    pub works_count: u64,
    pub cited_by_count: u64,
    pub summary_stats: Option<AuthorSummaryStats>,
    pub homepage_url: Option<String>,
    pub image_url: Option<String>,
    pub image_thumbnail_url: Option<String>,
    pub updated_date: Option<String>,
}

/// An associated institution (parent / child / related).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AssociatedInstitution {
    pub id: String,
    pub display_name: String,
    pub ror: Option<String>,
    pub country_code: Option<String>,
    pub type_: Option<String>,
    pub relationship: Option<String>,
}

// ===========================================================================
// Topic
// ===========================================================================

/// A research topic classification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Topic {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub works_count: u64,
    pub cited_by_count: u64,
    pub subfield: Option<DehydratedEntity>,
    pub field: Option<DehydratedEntity>,
    pub domain: Option<DehydratedEntity>,
    pub updated_date: Option<String>,
}

// ===========================================================================
// Funder
// ===========================================================================

/// A funding organisation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Funder {
    pub id: String,
    pub display_name: String,
    pub alternate_titles: Vec<String>,
    pub country_code: Option<String>,
    pub type_: Option<String>,
    pub ror: Option<String>,
    pub works_count: u64,
    pub cited_by_count: u64,
    pub grants_count: Option<u64>,
    pub homepage_url: Option<String>,
    pub updated_date: Option<String>,
}

// ===========================================================================
// Autocomplete
// ===========================================================================

/// Response from the `/autocomplete/{entity}` endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutocompleteResponse {
    pub meta: Meta,
    pub results: Vec<AutocompleteResult>,
}

/// A single autocomplete suggestion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutocompleteResult {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub entity_type: Option<String>,
    pub external_id: Option<String>,
    pub relevance_score: Option<f64>,
    pub work_count: Option<u64>,
    pub cited_by_count: Option<u64>,
}

// ===========================================================================
// Unit tests (serde deserialization — no network)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_minimal_work() {
        let json = r#"{
            "id": "https://openalex.org/W2741809807",
            "doi": "https://doi.org/10.7717/peerj.4375",
            "display_name": "The state of OA",
            "publication_year": 2018,
            "cited_by_count": 100,
            "type": "article"
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert_eq!(work.id, "https://openalex.org/W2741809807");
        assert_eq!(
            work.doi.as_deref(),
            Some("https://doi.org/10.7717/peerj.4375")
        );
        assert_eq!(work.title_or_name(), Some("The state of OA"));
        assert_eq!(work.publication_year, Some(2018));
        assert_eq!(work.cited_by_count, 100);
        assert_eq!(work.type_.as_deref(), Some("article"));
    }

    #[test]
    fn deserialize_work_with_authorships() {
        let json = r#"{
            "id": "https://openalex.org/W123",
            "cited_by_count": 0,
            "authorships": [
                {
                    "author_position": "first",
                    "author": {
                        "id": "https://openalex.org/A123",
                        "display_name": "Jane Doe",
                        "orcid": "https://orcid.org/0000-0001-2345-6789"
                    },
                    "institutions": [
                        {
                            "id": "https://openalex.org/I123",
                            "display_name": "MIT",
                            "country_code": "US"
                        }
                    ]
                }
            ]
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert_eq!(work.authorships.len(), 1);
        let a = &work.authorships[0];
        assert_eq!(a.author_position.as_deref(), Some("first"));
        assert_eq!(a.author.display_name.as_deref(), Some("Jane Doe"));
        assert_eq!(
            a.author.orcid.as_deref(),
            Some("https://orcid.org/0000-0001-2345-6789")
        );
        assert_eq!(a.institutions.len(), 1);
        assert_eq!(a.institutions[0].display_name.as_deref(), Some("MIT"));
    }

    #[test]
    fn deserialize_work_with_open_access() {
        let json = r#"{
            "id": "https://openalex.org/W456",
            "open_access": {
                "is_oa": true,
                "oa_status": "gold",
                "oa_url": "https://example.com/paper.pdf"
            }
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert!(work.open_access.is_oa);
        assert_eq!(work.open_access.oa_status.as_deref(), Some("gold"));
        assert_eq!(
            work.open_access.oa_url.as_deref(),
            Some("https://example.com/paper.pdf")
        );
    }

    #[test]
    fn deserialize_abstract_inverted_index() {
        let json = r#"{
            "id": "https://openalex.org/W789",
            "abstract_inverted_index": {
                "Hello": [0],
                "world": [1],
                "foo": [2, 5],
                "bar": [3]
            }
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        let abs = work.abstract_text().unwrap();
        assert_eq!(abs, "Hello world foo bar foo");
    }

    #[test]
    fn deserialize_list_response() {
        let json = r#"{
            "meta": {
                "count": 2,
                "per_page": 25
            },
            "results": [
                {"id": "https://openalex.org/W1", "cited_by_count": 10},
                {"id": "https://openalex.org/W2", "cited_by_count": 20}
            ],
            "group_by": []
        }"#;
        let resp: ListResponse<Work> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.meta.count, 2);
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].id, "https://openalex.org/W1");
        assert!(resp.group_by.is_empty());
    }

    #[test]
    fn deserialize_group_by_response() {
        let json = r#"{
            "meta": {"count": 100},
            "results": [],
            "group_by": [
                {"key": "article", "key_display_name": "Article", "count": 80},
                {"key": "book", "key_display_name": "Book", "count": 20}
            ]
        }"#;
        let resp: ListResponse<Work> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.group_by.len(), 2);
        assert_eq!(resp.group_by[0].key, "article");
        assert_eq!(resp.group_by[0].count, 80);
    }

    #[test]
    fn deserialize_autocomplete_response() {
        let json = r#"{
            "meta": {"count": 2},
            "results": [
                {
                    "id": "https://openalex.org/W123",
                    "display_name": "Machine Learning",
                    "entity_type": "work",
                    "relevance_score": 0.95,
                    "work_count": 5000
                }
            ]
        }"#;
        let resp: AutocompleteResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(
            resp.results[0].display_name.as_deref(),
            Some("Machine Learning")
        );
        assert_eq!(resp.results[0].relevance_score, Some(0.95));
    }

    #[test]
    fn deserialize_author() {
        let json = r#"{
            "id": "https://openalex.org/A5023888391",
            "display_name": "Albert Einstein",
            "orcid": "https://orcid.org/0000-0001-6187-6610",
            "works_count": 350,
            "cited_by_count": 50000,
            "summary_stats": {
                "h_index": 80,
                "i10_index": 200
            }
        }"#;
        let author: Author = serde_json::from_str(json).unwrap();
        assert_eq!(author.display_name, "Albert Einstein");
        assert_eq!(author.works_count, 350);
        assert_eq!(author.summary_stats.unwrap().h_index, Some(80));
    }

    #[test]
    fn deserialize_source() {
        let json = r#"{
            "id": "https://openalex.org/S123",
            "display_name": "Nature",
            "issn_l": "0028-0836",
            "issn": ["0028-0836"],
            "works_count": 100000,
            "is_oa": false
        }"#;
        let source: Source = serde_json::from_str(json).unwrap();
        assert_eq!(source.display_name, "Nature");
        assert_eq!(source.issn.len(), 1);
        assert!(!source.is_oa);
    }

    #[test]
    fn deserialize_institution() {
        let json = r#"{
            "id": "https://openalex.org/I136335617",
            "display_name": "Harvard University",
            "ror": "https://ror.org/03vek6s52",
            "country_code": "US",
            "type": "education",
            "works_count": 500000
        }"#;
        let inst: Institution = serde_json::from_str(json).unwrap();
        assert_eq!(inst.display_name, "Harvard University");
        assert_eq!(inst.country_code.as_deref(), Some("US"));
        assert_eq!(inst.works_count, 500000);
    }

    #[test]
    fn deserialize_topic() {
        let json = r#"{
            "id": "https://openalex.org/T10001",
            "display_name": "Machine Learning",
            "description": "Algorithms and statistical models",
            "works_count": 200000,
            "keywords": ["neural networks", "deep learning"]
        }"#;
        let topic: Topic = serde_json::from_str(json).unwrap();
        assert_eq!(topic.display_name, "Machine Learning");
        assert_eq!(topic.keywords.len(), 2);
        assert_eq!(topic.works_count, 200000);
    }

    #[test]
    fn empty_work_defaults() {
        let json = r#"{"id": "https://openalex.org/W1"}"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert_eq!(work.id, "https://openalex.org/W1");
        assert_eq!(work.cited_by_count, 0);
        assert!(!work.is_retracted);
        assert!(work.authorships.is_empty());
        assert!(work.abstract_text().is_none());
    }

    #[test]
    fn deserialize_meta_with_cursor() {
        let json = r#"{
            "count": 286750097,
            "db_response_time_ms": 152.5,
            "page": 1,
            "per_page": 25,
            "next_cursor": "IlsxNzE1MDc1MjYwMDAwLCAnaSdd",
            "cost_usd": 0.0001
        }"#;
        let meta: Meta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.count, 286750097);
        assert!(meta.next_cursor.is_some());
        assert_eq!(meta.cost_usd, Some(0.0001));
    }

    #[test]
    fn deserialize_work_with_topics_and_mesh() {
        let json = r#"{
            "id": "https://openalex.org/W999",
            "topics": [
                {
                    "id": "https://openalex.org/T10100",
                    "display_name": "Cancer Research",
                    "score": 0.95
                }
            ],
            "mesh": [
                {
                    "descriptor_ui": "D009369",
                    "descriptor_name": "Neoplasms",
                    "is_major_topic": true
                }
            ]
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert_eq!(work.topics.len(), 1);
        assert_eq!(work.topics[0].display_name, "Cancer Research");
        assert_eq!(work.mesh.len(), 1);
        assert_eq!(work.mesh[0].descriptor_name.as_deref(), Some("Neoplasms"));
    }

    #[test]
    fn deserialize_counts_by_year() {
        let json = r#"{
            "id": "https://openalex.org/W1",
            "counts_by_year": [
                {"year": 2024, "cited_by_count": 10, "works_count": 5},
                {"year": 2023, "cited_by_count": 20}
            ]
        }"#;
        let work: Work = serde_json::from_str(json).unwrap();
        assert_eq!(work.counts_by_year.len(), 2);
        assert_eq!(work.counts_by_year[0].year, 2024);
        assert_eq!(work.counts_by_year[0].cited_by_count, 10);
    }
}
