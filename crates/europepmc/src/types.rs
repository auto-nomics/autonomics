use serde::{Deserialize, Serialize};

// ===========================================================================
// Request types
// ===========================================================================

/// Result type determines the level of detail returned by the search and
/// article endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultType {
    /// Returns a list of IDs and sources only.
    Idlist,
    /// Returns key metadata (default).
    #[default]
    Lite,
    /// Returns full metadata including abstract, full-text links, MeSH terms.
    Core,
}

impl ResultType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idlist => "idlist",
            Self::Lite => "lite",
            Self::Core => "core",
        }
    }
}

/// Europe PMC data source codes.
///
/// Each article in Europe PMC has a three-letter source code. The most common
/// is `MED` (PubMed/MEDLINE). See the [data sources help page](
/// https://europepmc.org/Faq#sources) for details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Agr,
    Cba,
    Cit,
    Ctx,
    Eth,
    Hir,
    Med,
    Nbk,
    Pat,
    Pmc,
    Ppr,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agr => "AGR",
            Self::Cba => "CBA",
            Self::Cit => "CIT",
            Self::Ctx => "CTX",
            Self::Eth => "ETH",
            Self::Hir => "HIR",
            Self::Med => "MED",
            Self::Nbk => "NBK",
            Self::Pat => "PAT",
            Self::Pmc => "PMC",
            Self::Ppr => "PPR",
        }
    }

    /// Parse a source code string, case-insensitive.
    pub fn parse_source(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "AGR" => Some(Self::Agr),
            "CBA" => Some(Self::Cba),
            "CIT" => Some(Self::Cit),
            "CTX" => Some(Self::Ctx),
            "ETH" => Some(Self::Eth),
            "HIR" => Some(Self::Hir),
            "MED" => Some(Self::Med),
            "NBK" => Some(Self::Nbk),
            "PAT" => Some(Self::Pat),
            "PMC" => Some(Self::Pmc),
            "PPR" => Some(Self::Ppr),
            _ => None,
        }
    }
}

/// Parameters for [`EuropePmcClient::search`](crate::EuropePmcClient::search).
#[derive(Debug, Clone, Serialize)]
pub struct SearchRequest {
    /// Europe PMC query expression.
    ///
    /// Uses field prefixes: `TITLE:` (title), `AUTH:` (author), `ABSTRACT:`
    /// (abstract), `KEYWORD:` (keyword), `JOURNAL:` (journal), `MESH:`
    /// (MeSH term), `AFF:` (affiliation), `PUB_TYPE:` (publication type),
    /// `PUB_YEAR:` (publication year). Bare terms search all fields.
    ///
    /// Boolean operators (uppercase): `AND`, `OR`, `NOT`. Group with
    /// parentheses. Phrase-quote with double quotes.
    ///
    /// Example: `AUTH:Smith AND TITLE:p53`.
    pub query: String,

    /// Level of detail in the response.
    pub result_type: ResultType,

    /// Enable MeSH synonym expansion.
    pub synonym: bool,

    /// Cursor for deep-pagination. Use `"*"` for the first page, then pass
    /// back `nextCursorMark` from the previous response.
    pub cursor_mark: Option<String>,

    /// Number of results per page (default 25, max 1000).
    pub page_size: Option<u32>,

    /// Sort field and order, e.g. `"CITED asc"`, `"P_PDATE_D desc"`.
    pub sort: Option<String>,
}

impl SearchRequest {
    /// Convenience constructor for a simple query with lite results.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            result_type: ResultType::Lite,
            synonym: false,
            cursor_mark: None,
            page_size: None,
            sort: None,
        }
    }

    /// Set the result type.
    pub fn result_type(mut self, rt: ResultType) -> Self {
        self.result_type = rt;
        self
    }

    /// Enable MeSH synonym expansion.
    pub fn synonym(mut self, yes: bool) -> Self {
        self.synonym = yes;
        self
    }

    /// Set the cursor mark for pagination.
    pub fn cursor_mark(mut self, c: impl Into<String>) -> Self {
        self.cursor_mark = Some(c.into());
        self
    }

    /// Set the page size.
    pub fn page_size(mut self, n: u32) -> Self {
        self.page_size = Some(n);
        self
    }

    /// Set the sort expression.
    pub fn sort(mut self, s: impl Into<String>) -> Self {
        self.sort = Some(s.into());
        self
    }
}

/// Parameters for pagination of references / citations.
#[derive(Debug, Clone, Default)]
pub struct PageParams {
    /// Page number (1-based, default 1).
    pub page: Option<u32>,
    /// Results per page (default 25, max 1000).
    pub page_size: Option<u32>,
}

impl PageParams {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn page(mut self, n: u32) -> Self {
        self.page = Some(n);
        self
    }
    pub fn page_size(mut self, n: u32) -> Self {
        self.page_size = Some(n);
        self
    }
}

// ===========================================================================
// Response types — search
// ===========================================================================

/// Parsed response from the article endpoint.
///
/// Unlike [`SearchResponse`], the article endpoint returns a single `result`
/// object (not wrapped in `resultList`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArticleResponse {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "hitCount", default)]
    pub hit_count: u64,
    #[serde(default)]
    pub request: serde_json::Value,
    /// The single article result (present when the article is found).
    #[serde(default)]
    pub result: Option<SearchResult>,
}

/// Parsed response from the search endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResponse {
    /// API version.
    #[serde(default)]
    pub version: String,
    /// Total number of matching results.
    #[serde(rename = "hitCount", default)]
    pub hit_count: u64,
    /// Cursor mark for the next page of results.
    #[serde(rename = "nextCursorMark", default)]
    pub next_cursor_mark: Option<String>,
    /// Echo of the original request parameters.
    #[serde(default)]
    pub request: RequestEcho,
    /// Result list wrapper.
    #[serde(rename = "resultList", default)]
    pub result_list: ResultList,
}

/// The `resultList` object from a search response.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ResultList {
    /// The actual result entries.
    #[serde(default, rename = "result")]
    pub results: Vec<SearchResult>,
}

/// Echo of request parameters.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RequestEcho {
    #[serde(rename = "queryString", default)]
    pub query_string: String,
    #[serde(rename = "resultType", default)]
    pub result_type: String,
    #[serde(rename = "cursorMark", default)]
    pub cursor_mark: String,
    #[serde(rename = "pageSize", default)]
    pub page_size: u32,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub synonym: bool,
}

/// A single search result entry — **lite** fields.
///
/// When `resultType=core`, additional fields are populated (abstract text,
/// full author list, MeSH headings, etc.). All such extra fields are optional
/// and deserialise from the same struct so the caller does not need to switch
/// on response shape.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchResult {
    /// Article identifier within the source (e.g. PubMed ID).
    #[serde(default)]
    pub id: String,
    /// Three-letter source code (e.g. `"MED"`, `"PMC"`, `"PPR"`).
    #[serde(default)]
    pub source: String,
    /// PubMed ID if the article is from MED.
    #[serde(default)]
    pub pmid: Option<String>,
    /// DOI.
    #[serde(default)]
    pub doi: Option<String>,
    /// Article title.
    #[serde(default)]
    pub title: String,
    /// Comma-separated author string (e.g. `"Fang Y, Zhang T, Ye Q."`).
    #[serde(default, rename = "authorString")]
    pub author_string: Option<String>,
    /// Full author list (core resultType only).
    #[serde(default, rename = "authorList")]
    pub author_list: Option<AuthorListWrapper>,
    /// Journal title (lite field).
    #[serde(default, rename = "journalTitle")]
    pub journal_title: Option<String>,
    /// Detailed journal info (core resultType only).
    #[serde(default, rename = "journalInfo")]
    pub journal_info: Option<JournalInfo>,
    /// Publication year (string in lite, e.g. `"2024"`).
    #[serde(default, rename = "pubYear")]
    pub pub_year: Option<String>,
    /// Journal volume.
    #[serde(default, rename = "journalVolume")]
    pub journal_volume: Option<String>,
    /// Journal issue.
    #[serde(default)]
    pub issue: Option<String>,
    /// Page info (e.g. `"100-110"`).
    #[serde(default, rename = "pageInfo")]
    pub page_info: Option<String>,
    /// ISSN string (may contain multiple, semicolon-separated).
    #[serde(default, rename = "journalIssn")]
    pub journal_issn: Option<String>,
    /// Publication type string (semicolon-separated).
    #[serde(default, rename = "pubType")]
    pub pub_type: Option<String>,
    /// Abstract text (core resultType only).
    #[serde(default, rename = "abstractText")]
    pub abstract_text: Option<String>,
    /// First affiliation (core resultType only).
    #[serde(default)]
    pub affiliation: Option<String>,
    /// Language code (e.g. `"eng"`).
    #[serde(default)]
    pub language: Option<String>,
    /// Open access status: `"Y"` or `"N"`.
    #[serde(default, rename = "isOpenAccess")]
    pub is_open_access: Option<String>,
    /// Cited-by count.
    #[serde(default, rename = "citedByCount")]
    pub cited_by_count: Option<u64>,
    /// MeSH headings (core resultType only).
    #[serde(default, rename = "meshHeadingList")]
    pub mesh_heading_list: Option<MeshHeadingList>,
    /// Publication types list (core resultType only).
    #[serde(default, rename = "pubTypeList")]
    pub pub_type_list: Option<PubTypeList>,
    /// First publication date (`YYYY-MM-DD`).
    #[serde(default, rename = "firstPublicationDate")]
    pub first_publication_date: Option<String>,
    /// First index date in Europe PMC (`YYYY-MM-DD`).
    #[serde(default, rename = "firstIndexDate")]
    pub first_index_date: Option<String>,
    /// Has full text in Europe PMC.
    #[serde(default, rename = "inEPMC")]
    pub in_epmc: Option<String>,
    /// Has full text in PMC.
    #[serde(default, rename = "inPMC")]
    pub in_pmc: Option<String>,
    /// Has PDF available.
    #[serde(default, rename = "hasPDF")]
    pub has_pdf: Option<String>,
}

// ---------------------------------------------------------------------------
// Author (core)
// ---------------------------------------------------------------------------

/// Wrapper for the `authorList.author` JSON path.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorListWrapper {
    #[serde(default, rename = "author")]
    pub authors: Vec<AuthorDetail>,
}

/// A single author with structured name parts.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorDetail {
    /// Display name (e.g. `"Fang Y"`).
    #[serde(default, rename = "fullName")]
    pub full_name: String,
    /// First / given name.
    #[serde(default, rename = "firstName")]
    pub first_name: Option<String>,
    /// Last / family name.
    #[serde(default, rename = "lastName")]
    pub last_name: Option<String>,
    /// Initials.
    #[serde(default)]
    pub initials: Option<String>,
    /// Author ID (e.g. ORCID).
    #[serde(default, rename = "authorId")]
    pub author_id: Option<AuthorId>,
    /// Affiliations.
    #[serde(default, rename = "authorAffiliationDetailsList")]
    pub affiliation_details: Option<AffiliationDetailsList>,
}

/// An ORCID or other author identifier.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AuthorId {
    #[serde(default, rename = "type")]
    pub id_type: String,
    #[serde(default)]
    pub value: String,
}

/// Wrapper for author affiliations.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AffiliationDetailsList {
    #[serde(default, rename = "authorAffiliation")]
    pub affiliations: Vec<AffiliationEntry>,
}

/// A single affiliation entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AffiliationEntry {
    #[serde(default)]
    pub affiliation: String,
}

// ---------------------------------------------------------------------------
// Journal info (core)
// ---------------------------------------------------------------------------

/// Detailed journal metadata from `journalInfo`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct JournalInfo {
    #[serde(default)]
    pub issue: Option<String>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default, rename = "dateOfPublication")]
    pub date_of_publication: Option<String>,
    #[serde(default, rename = "monthOfPublication")]
    pub month_of_publication: Option<u8>,
    #[serde(default, rename = "yearOfPublication")]
    pub year_of_publication: Option<u16>,
    #[serde(default, rename = "printPublicationDate")]
    pub print_publication_date: Option<String>,
    #[serde(default)]
    pub journal: Option<Journal>,
}

/// Journal-level metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Journal {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "medlineAbbreviation")]
    pub medline_abbreviation: Option<String>,
    #[serde(default)]
    pub issn: Option<String>,
    #[serde(default)]
    pub essn: Option<String>,
    #[serde(default, rename = "isoabbreviation")]
    pub iso_abbreviation: Option<String>,
}

// ---------------------------------------------------------------------------
// MeSH headings (core)
// ---------------------------------------------------------------------------

/// Wrapper for `meshHeadingList.meshHeading`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MeshHeadingList {
    #[serde(default, rename = "meshHeading")]
    pub headings: Vec<MeshHeading>,
}

/// A single MeSH heading.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MeshHeading {
    #[serde(default, rename = "descriptorName")]
    pub descriptor_name: String,
    #[serde(default, rename = "majorTopic_YN")]
    pub major_topic: String,
}

/// Wrapper for `pubTypeList.pubType`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PubTypeList {
    #[serde(default, rename = "pubType")]
    pub types: Vec<String>,
}

// ===========================================================================
// Response types — references
// ===========================================================================

/// Parsed response from the references endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReferencesResponse {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "hitCount", default)]
    pub hit_count: u64,
    #[serde(default)]
    pub request: ReferenceRequestEcho,
    #[serde(default, rename = "referenceList")]
    pub reference_list: Option<ReferenceListWrapper>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ReferenceRequestEcho {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub off_set: u32,
    #[serde(default, rename = "pageSize")]
    pub page_size: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ReferenceListWrapper {
    #[serde(default, rename = "reference")]
    pub references: Vec<ReferenceEntry>,
}

/// A single reference entry (cited work).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ReferenceEntry {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, rename = "citationType")]
    pub citation_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "authorString")]
    pub author_string: Option<String>,
    #[serde(default, rename = "journalAbbreviation")]
    pub journal_abbreviation: Option<String>,
    #[serde(default)]
    pub issue: Option<String>,
    #[serde(default, rename = "pubYear")]
    pub pub_year: Option<u16>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default, rename = "pageInfo")]
    pub page_info: Option<String>,
    #[serde(default, rename = "citedOrder")]
    pub cited_order: Option<u32>,
}

// ===========================================================================
// Response types — citations
// ===========================================================================

/// Parsed response from the citations endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CitationsResponse {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "hitCount", default)]
    pub hit_count: u64,
    #[serde(default)]
    pub request: ReferenceRequestEcho,
    #[serde(default, rename = "citationList")]
    pub citation_list: Option<CitationListWrapper>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CitationListWrapper {
    #[serde(default, rename = "citation")]
    pub citations: Vec<CitationEntry>,
}

/// A single citation entry (citing work).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CitationEntry {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, rename = "citationType")]
    pub citation_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "authorString")]
    pub author_string: Option<String>,
    #[serde(default, rename = "journalAbbreviation")]
    pub journal_abbreviation: Option<String>,
    #[serde(default, rename = "pubYear")]
    pub pub_year: Option<u16>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub issue: Option<String>,
    #[serde(default, rename = "pageInfo")]
    pub page_info: Option<String>,
    #[serde(default, rename = "citedByCount")]
    pub cited_by_count: Option<u64>,
}

// ===========================================================================
// Response types — profile
// ===========================================================================

/// Parsed response from the profile endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileResponse {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub request: ProfileRequestEcho,
    #[serde(default, rename = "profileList")]
    pub profile_list: Option<ProfileList>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileRequestEcho {
    #[serde(default, rename = "queryString")]
    pub query_string: String,
    #[serde(default, rename = "profileType")]
    pub profile_type: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileList {
    #[serde(default, rename = "pubType")]
    pub pub_types: Option<Vec<ProfileEntry>>,
    #[serde(default, rename = "source")]
    pub sources: Option<Vec<ProfileEntry>>,
}

/// A single profile bucket (name + count).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub count: u64,
}
