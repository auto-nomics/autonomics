use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Identifier kinds for retrieval
// ---------------------------------------------------------------------------

/// The type of identifier used to retrieve an article from Embase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetrievalId {
    /// Digital Object Identifier.
    Doi,
    /// Publication Item Identifier (Elsevier PII).
    Pii,
    /// PubMed Identifier.
    PubmedId,
    /// MEDLINE identifier.
    Medline,
    /// Embase Accession Number.
    Embase,
    /// Local Unique Identifier (Embase internal).
    Lui,
}

impl RetrievalId {
    /// URL path segment for this identifier type.
    pub fn path_segment(self) -> &'static str {
        match self {
            RetrievalId::Doi => "doi",
            RetrievalId::Pii => "pii",
            RetrievalId::PubmedId => "pubmed_id",
            RetrievalId::Medline => "medline",
            RetrievalId::Embase => "embase",
            RetrievalId::Lui => "lui",
        }
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Parameters for [`EmbaseClient::search`](crate::EmbaseClient::search).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchRequest {
    /// Embase CommandLanguage query string
    /// (e.g. `"'heart attack':ti,ab AND aspirin:ti,ab"`).
    pub query: String,
    /// Maximum number of results to return (default 25).
    pub count: Option<u32>,
    /// Start position (1-based, default 1).
    pub start: Option<u32>,
    /// Sort order: `"relevance"` (default), `"entrydate"`, or `"publicationyear"`.
    pub sort: Option<String>,
    /// Encoded alert ID (base64). Alternative to `query`.
    pub alert_id: Option<String>,
}

impl SearchRequest {
    /// Convenience constructor for a simple query search.
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_owned(),
            count: None,
            start: None,
            sort: None,
            alert_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Response types — search
// ---------------------------------------------------------------------------

/// Parsed response from [`EmbaseClient::search`](crate::EmbaseClient::search).
///
/// The Embase API follows the Elsevier Atom-JSON convention with
/// `opensearch:*` pagination fields and an `entry` array.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResponse {
    /// Total number of matching records across all pages.
    #[serde(rename = "opensearch:totalResults")]
    pub total_results: String,

    /// Start index of the current page (as a string).
    #[serde(rename = "opensearch:startIndex", default)]
    pub start_index: String,

    /// Number of results per page (as a string).
    #[serde(rename = "opensearch:itemsPerPage", default)]
    pub items_per_page: String,

    /// Result entries. Empty when no results or an error occurred.
    #[serde(default)]
    pub entry: Vec<SearchEntry>,

    /// Optional error payload (present on API-level errors).
    #[serde(default)]
    pub error: Option<ErrorPayload>,
}

/// A single search-result entry (Atom-style with Dublin Core and PRISM fields).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchEntry {
    /// Internal Embase identifier (e.g. `"doi:10.1016/..."` or `"EMBASE_..."`).
    #[serde(rename = "dc:identifier", default)]
    pub identifier: Option<String>,

    /// Article title.
    #[serde(rename = "dc:title", default)]
    pub title: String,

    /// First author / creator name.
    #[serde(rename = "dc:creator", default)]
    pub creator: Option<String>,

    /// All author names (pipe-separated string in the raw API).
    #[serde(rename = "dc:source", default)]
    pub source: Option<String>,

    /// DOI.
    #[serde(rename = "prism:doi", default)]
    pub doi: Option<String>,

    /// Publication (journal) name.
    #[serde(rename = "prism:publicationName", default)]
    pub publication_name: Option<String>,

    /// Volume.
    #[serde(rename = "prism:volume", default)]
    pub volume: Option<String>,

    /// Issue identifier.
    #[serde(rename = "prism:issueIdentifier", default)]
    pub issue_identifier: Option<String>,

    /// Cover date (e.g. `"2024-01-15"`).
    #[serde(rename = "prism:coverDate", default)]
    pub cover_date: Option<String>,

    /// Page range.
    #[serde(rename = "prism:pageRange", default)]
    pub page_range: Option<String>,

    /// ISSN (print).
    #[serde(rename = "prism:issn", default)]
    pub issn: Option<String>,

    /// E-ISSN.
    #[serde(rename = "prism:eIssn", default)]
    pub eissn: Option<String>,

    /// Abstract text.
    #[serde(rename = "dc:description", default)]
    pub description: Option<String>,

    /// Publication type / document type.
    #[serde(rename = "prism:aggregationType", default)]
    pub aggregation_type: Option<String>,

    /// API URL for the full record.
    #[serde(rename = "prism:url", default)]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types — retrieval
// ---------------------------------------------------------------------------

/// Parsed response from a retrieval API call.
///
/// The retrieval endpoints return a single `entry` object (not wrapped in
/// a `search-results` container).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetrievalResponse {
    /// The retrieved article entry.
    pub entry: SearchEntry,

    /// Optional error payload.
    #[serde(default)]
    pub error: Option<ErrorPayload>,
}

/// Elsevier API error payload (returned in the JSON body on failure).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ErrorPayload {
    /// Numeric status code from the API.
    #[serde(default)]
    pub status: Option<String>,
    /// Error detail text.
    #[serde(default)]
    pub detail: Option<String>,
}
