use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Sort field for arXiv search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum SortBy {
    /// Sort by arXiv's relevance score (default).
    #[default]
    Relevance,
    /// Sort by last-updated date.
    LastUpdatedDate,
    /// Sort by original submission date.
    SubmittedDate,
}

impl SortBy {
    /// Render to the arXiv API's `sortBy` parameter value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::LastUpdatedDate => "lastUpdatedDate",
            Self::SubmittedDate => "submittedDate",
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum SortOrder {
    /// Ascending (oldest first).
    Ascending,
    /// Descending (newest first, default).
    #[default]
    Descending,
}

impl SortOrder {
    /// Render to the arXiv API's `sortOrder` parameter value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// Parameters for [`ArxivClient::search`](crate::ArxivClient::search).
#[derive(Debug, Clone, Serialize)]
pub struct SearchRequest {
    /// arXiv search query expression.
    ///
    /// Uses arXiv field prefixes: `ti:` (title), `au:` (author), `abs:`
    /// (abstract), `cat:` (category), `all:` (all fields), `jr:` (journal
    /// reference), `co:` (comment).
    ///
    /// Boolean operators: `AND`, `OR`, `ANDNOT`. Group with parentheses.
    /// Phrases in double quotes.
    ///
    /// Example: `"au:Hinton AND ti:neural network"`.
    pub search_query: String,

    /// Maximum number of results to return (default 10, max 2000 per page).
    pub max_results: Option<u32>,

    /// 0-based start index for pagination.
    pub start: Option<u32>,

    /// Sort field.
    pub sort_by: Option<SortBy>,

    /// Sort direction.
    pub sort_order: Option<SortOrder>,
}

impl SearchRequest {
    /// Convenience constructor for a simple search query.
    pub fn new(search_query: impl Into<String>) -> Self {
        Self {
            search_query: search_query.into(),
            max_results: None,
            start: None,
            sort_by: None,
            sort_order: None,
        }
    }

    /// Set the maximum number of results to return.
    pub fn max_results(mut self, n: u32) -> Self {
        self.max_results = Some(n);
        self
    }

    /// Set the 0-based start index for pagination.
    pub fn start(mut self, n: u32) -> Self {
        self.start = Some(n);
        self
    }

    /// Set the sort field.
    pub fn sort_by(mut self, by: SortBy) -> Self {
        self.sort_by = Some(by);
        self
    }

    /// Set the sort direction.
    pub fn sort_order(mut self, order: SortOrder) -> Self {
        self.sort_order = Some(order);
        self
    }
}

/// Parameters for [`ArxivClient::fetch_by_id`](crate::ArxivClient::fetch_by_id).
#[derive(Debug, Clone, Serialize)]
pub struct FetchRequest {
    /// Comma-separated arXiv IDs (e.g. `"2401.12345,2309.01234v2"`).
    ///
    /// Old-style IDs like `"cond-mat/0207270"` are also accepted. Append
    /// `vN` to fetch a specific version.
    pub id_list: String,

    /// Maximum number of results to return.
    pub max_results: Option<u32>,

    /// 0-based start index.
    pub start: Option<u32>,
}

impl FetchRequest {
    /// Convenience constructor for one or more comma-separated arXiv IDs.
    pub fn new(id_list: impl Into<String>) -> Self {
        Self {
            id_list: id_list.into(),
            max_results: None,
            start: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// A single arXiv entry in the Atom feed, parsed into typed Rust.
///
/// This mirrors the raw data returned by the arXiv API before conversion to
/// [`bib_types::Article`]. Use [`crate::convert::atom_to_articles`] to convert
/// a list of these into the shared `Article` model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArxivEntry {
    /// arXiv ID with version if specified (e.g. `"2401.12345"` or
    /// `"2401.12345v2"`). Stripped of the `http://arxiv.org/abs/` prefix.
    pub arxiv_id: String,

    /// Article title (whitespace collapsed).
    pub title: String,

    /// Abstract / summary text.
    pub summary: Option<String>,

    /// Authors in display order.
    pub authors: Vec<ArxivAuthor>,

    /// ISO-8601 publication date (first submission), e.g.
    /// `"2024-01-15T00:00:00Z"`.
    pub published: Option<String>,

    /// ISO-8601 last-updated date (latest version submission).
    pub updated: Option<String>,

    /// Primary arXiv category (e.g. `"cs.LG"`, `"q-bio.GN"`).
    pub primary_category: Option<String>,

    /// All subject categories assigned to the article.
    pub categories: Vec<String>,

    /// DOI if provided by the submitter.
    pub doi: Option<String>,

    /// Journal reference if provided.
    pub journal_ref: Option<String>,

    /// Author comment (e.g. page/figure counts).
    pub comment: Option<String>,

    /// PDF download URL.
    pub pdf_url: Option<String>,

    /// Abstract page URL.
    pub abs_url: Option<String>,
}

/// An author in an arXiv entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArxivAuthor {
    /// Author display name as provided (e.g. `"Yann LeCun"`).
    pub name: String,
    /// Affiliation if the author provided one.
    pub affiliation: Option<String>,
}

/// Parsed response from an arXiv API query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    /// Total number of matching results (from OpenSearch `totalResults`).
    pub total_results: u64,
    /// 0-based index of the first result returned.
    pub start_index: u64,
    /// Number of results returned in this page.
    pub items_per_page: u64,
    /// The parsed entries.
    pub entries: Vec<ArxivEntry>,
}
