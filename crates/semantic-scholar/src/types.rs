use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ===========================================================================
// Paper types
// ===========================================================================

/// A paper in the Semantic Scholar Academic Graph.
///
/// Fields are optional because the API only returns requested `fields`. When
/// no `fields` param is sent, `paperId` and `title` are always present.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Paper {
    /// S2 unique string identifier (always included).
    #[serde(default, rename = "paperId")]
    pub paper_id: String,

    /// Second unique numeric identifier.
    #[serde(default, rename = "corpusId")]
    pub corpus_id: Option<i64>,

    /// URL on the Semantic Scholar website.
    #[serde(default)]
    pub url: Option<String>,

    /// Paper title.
    #[serde(default)]
    pub title: Option<String>,

    /// Normalized venue name.
    #[serde(default)]
    pub venue: Option<String>,

    /// Year of publication.
    #[serde(default)]
    pub year: Option<i32>,

    /// Authors list (up to 500).
    #[serde(default)]
    pub authors: Vec<PaperAuthor>,

    /// External catalog IDs (DOI, ArXiv, PubMed, etc.).
    #[serde(default, rename = "externalIds")]
    pub external_ids: Option<ExternalIds>,

    /// Paper abstract.
    #[serde(default)]
    pub abstract_text: Option<String>,

    /// Total number of papers referenced by this paper.
    #[serde(default, rename = "referenceCount")]
    pub reference_count: Option<i32>,

    /// Total number of citations S2 has found for this paper.
    #[serde(default, rename = "citationCount")]
    pub citation_count: Option<i32>,

    /// Number of influential citations.
    #[serde(default, rename = "influentialCitationCount")]
    pub influential_citation_count: Option<i32>,

    /// Whether the paper is open access.
    #[serde(default, rename = "isOpenAccess")]
    pub is_open_access: Option<bool>,

    /// Open access PDF link and status.
    #[serde(default, rename = "openAccessPdf")]
    pub open_access_pdf: Option<OpenAccessPdf>,

    /// High-level academic categories from external sources.
    #[serde(default, rename = "fieldsOfStudy")]
    pub fields_of_study: Option<Vec<String>>,

    /// S2-classified fields of study with source.
    #[serde(default, rename = "s2FieldsOfStudy")]
    pub s2_fields_of_study: Option<Vec<S2FieldOfStudy>>,

    /// Publication types (Journal Article, Review, etc.).
    #[serde(default, rename = "publicationTypes")]
    pub publication_types: Option<Vec<String>>,

    /// Publication date (YYYY-MM-DD).
    #[serde(default, rename = "publicationDate")]
    pub publication_date: Option<String>,

    /// Journal name, volume, and pages.
    #[serde(default)]
    pub journal: Option<Journal>,

    /// Auto-generated TLDR summary.
    #[serde(default)]
    pub tldr: Option<Tldr>,

    /// SPECTER embedding.
    #[serde(default)]
    pub embedding: Option<Embedding>,
}

/// A lightweight author reference inside a paper.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PaperAuthor {
    /// S2 author ID.
    #[serde(default, rename = "authorId")]
    pub author_id: Option<String>,

    /// Author name.
    #[serde(default)]
    pub name: Option<String>,
}

/// External IDs from other sources.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExternalIds {
    #[serde(default, rename = "DOI")]
    pub doi: Option<String>,
    #[serde(default, rename = "ArXiv")]
    pub arxiv: Option<String>,
    #[serde(default, rename = "PubMed")]
    pub pubmed: Option<String>,
    #[serde(default, rename = "PubMedCentral")]
    pub pubmed_central: Option<String>,
    #[serde(default, rename = "MAG")]
    pub mag: Option<String>,
    #[serde(default, rename = "ACL")]
    pub acl: Option<String>,
    #[serde(default, rename = "DBLP")]
    pub dblp: Option<String>,
    #[serde(default, rename = "CorpusId")]
    pub corpus_id: Option<i64>,
}

/// Open access PDF info.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OpenAccessPdf {
    /// URL to the open access PDF.
    #[serde(default)]
    pub url: Option<String>,
    /// OA status (green, gold, hybrid, bronze, closed).
    #[serde(default)]
    pub status: Option<String>,
}

/// Journal metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Journal {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub pages: Option<String>,
}

/// Auto-generated TLDR.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Tldr {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

/// SPECTER paper embedding.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Embedding {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub vector: Option<Vec<f64>>,
}

/// S2 field of study with source.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct S2FieldOfStudy {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

// ===========================================================================
// Search response types
// ===========================================================================

/// Response from `/paper/search` (relevance search).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PaperSearchResponse {
    /// Approximate total matching results.
    #[serde(default)]
    pub total: i64,

    /// Starting position of this batch.
    #[serde(default)]
    pub offset: i64,

    /// Starting position of the next batch (absent if no more).
    #[serde(default)]
    pub next: Option<i64>,

    /// Matching papers.
    #[serde(default)]
    pub data: Vec<Paper>,
}

/// Response from `/paper/search/bulk`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PaperBulkResponse {
    /// Approximate total matching results.
    #[serde(default)]
    pub total: i64,

    /// Continuation token for fetching the next batch.
    #[serde(default)]
    pub token: Option<String>,

    /// Matching papers.
    #[serde(default)]
    pub data: Vec<Paper>,
}

// ===========================================================================
// Citation / reference types
// ===========================================================================

/// Response from `/paper/{id}/citations`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CitationBatchResponse {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub data: Vec<CitationEntry>,
}

/// A single citation (a paper that cites the target paper).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CitationEntry {
    /// Snippets where the reference is mentioned.
    #[serde(default)]
    pub contexts: Option<Vec<String>>,

    /// Citation intents (methodology, background, result).
    #[serde(default)]
    pub intents: Option<Vec<String>>,

    /// Whether this is an influential citation.
    #[serde(default, rename = "isInfluential")]
    pub is_influential: Option<bool>,

    /// The citing paper.
    #[serde(default, rename = "citingPaper")]
    pub citing_paper: Paper,
}

/// Response from `/paper/{id}/references`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ReferenceBatchResponse {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub data: Vec<ReferenceEntry>,
}

/// A single reference (a paper cited by the target paper).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ReferenceEntry {
    #[serde(default)]
    pub contexts: Option<Vec<String>>,
    #[serde(default)]
    pub intents: Option<Vec<String>>,
    #[serde(default, rename = "isInfluential")]
    pub is_influential: Option<bool>,
    /// The cited paper.
    #[serde(default, rename = "citedPaper")]
    pub cited_paper: Paper,
}

// ===========================================================================
// Author types
// ===========================================================================

/// Response from `/author/search`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorSearchResponse {
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub data: Vec<Author>,
}

/// An author in the Semantic Scholar Academic Graph.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Author {
    /// S2 unique author ID.
    #[serde(default, rename = "authorId")]
    pub author_id: String,

    /// Author name.
    #[serde(default)]
    pub name: Option<String>,

    /// URL on the Semantic Scholar website.
    #[serde(default)]
    pub url: Option<String>,

    /// Author aliases.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,

    /// Author affiliations.
    #[serde(default)]
    pub affiliations: Option<Vec<String>>,

    /// Author homepage.
    #[serde(default)]
    pub homepage: Option<String>,

    /// External IDs (ORCID, DBLP).
    #[serde(default, rename = "externalIds")]
    pub external_ids: Option<BTreeMap<String, String>>,

    /// Total publications count.
    #[serde(default, rename = "paperCount")]
    pub paper_count: Option<i64>,

    /// Total citations count.
    #[serde(default, rename = "citationCount")]
    pub citation_count: Option<i64>,

    /// H-index.
    #[serde(default, rename = "hIndex")]
    pub h_index: Option<i64>,
}

/// Response from `/author/{id}/papers`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorPapersResponse {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub data: Vec<Paper>,
}

// ===========================================================================
// Recommendations
// ===========================================================================

/// Response from `/recommendations/v1/papers/forpaper/{id}`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RecommendationsResponse {
    #[serde(default, rename = "recommendedPapers")]
    pub recommended_papers: Vec<Paper>,
}

// ===========================================================================
// Default field sets
// ===========================================================================

/// Default fields requested for paper search and details.
pub const DEFAULT_PAPER_FIELDS: &str =
    "paperId,title,abstract,year,venue,authors,externalIds,referenceCount,\
     citationCount,influentialCitationCount,isOpenAccess,openAccessPdf,\
     fieldsOfStudy,publicationTypes,publicationDate,journal,tldr";

/// Fields requested for citations/references (paper-level subset).
pub const DEFAULT_CITATION_FIELDS: &str =
    "paperId,title,abstract,year,venue,authors,citationCount,\
     isOpenAccess,openAccessPdf,externalIds,contexts,intents,isInfluential";

/// Default fields requested for author search and details.
pub const DEFAULT_AUTHOR_FIELDS: &str =
    "authorId,name,url,affiliations,homepage,paperCount,citationCount,hIndex";
