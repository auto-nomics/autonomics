use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Server selector
// ---------------------------------------------------------------------------

/// Which preprint server to query.
///
/// Both `api.medrxiv.org` and `api.biorxiv.org` serve the same API; the
/// `{server}` path segment selects the content pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Server {
    /// medRxiv — health sciences preprints.
    #[default]
    Medrxiv,
    /// bioRxiv — biology preprints.
    Biorxiv,
}

impl Server {
    /// URL path segment for the server.
    pub fn segment(self) -> &'static str {
        match self {
            Server::Medrxiv => "medrxiv",
            Server::Biorxiv => "biorxiv",
        }
    }
}

impl std::fmt::Display for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.segment())
    }
}

impl std::str::FromStr for Server {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "medrxiv" => Ok(Server::Medrxiv),
            "biorxiv" => Ok(Server::Biorxiv),
            other => Err(format!(
                "unknown server '{other}', expected 'medrxiv' or 'biorxiv'"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Interval — the "range" selector in details/pub URLs
// ---------------------------------------------------------------------------

/// Posting-date interval for browsing papers.
///
/// The bioRxiv/medRxiv API has **no keyword search** — it only supports
/// browsing by date range or recency. [`Interval`] encodes the three
/// accepted forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interval {
    /// The *N* most recent posts (path segment: `"N"`).
    Recent(u32),
    /// Posts from the last *N* days (path segment: `"Nd"`).
    Days(u32),
    /// Posts within an inclusive `[from, to]` date range
    /// (path segments: `"YYYY-MM-DD/YYYY-MM-DD"`).
    DateRange { from: NaiveDate, to: NaiveDate },
}

impl Interval {
    /// Render to the URL path segment(s) — one or two segments joined by `/`.
    pub fn path(&self) -> String {
        match self {
            Interval::Recent(n) => n.to_string(),
            Interval::Days(n) => format!("{n}d"),
            Interval::DateRange { from, to } => format!("{from}/{to}"),
        }
    }

    /// Create a date-range interval from a pair of `YYYY-MM-DD` strings.
    pub fn date_range(from: &str, to: &str) -> Result<Self, chrono::ParseError> {
        Ok(Interval::DateRange {
            from: NaiveDate::parse_from_str(from, "%Y-%m-%d")?,
            to: NaiveDate::parse_from_str(to, "%Y-%m-%d")?,
        })
    }
}

// ---------------------------------------------------------------------------
// Response envelope
// ---------------------------------------------------------------------------

/// Top-level JSON envelope shared by all API responses.
///
/// ```json
/// { "messages": [ … ], "collection": [ … ] }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>", serialize = "T: Serialize"))]
pub struct ApiResponse<T> {
    #[serde(default)]
    pub messages: Vec<Message>,
    pub collection: Vec<T>,
}

// ---------------------------------------------------------------------------
// Loose deserialiser — the API returns some numeric fields as JSON strings
// (e.g. `"total": "60"` alongside `"count": 60`). This helper accepts both.
// ---------------------------------------------------------------------------

fn deserialize_optional_u64_loose<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(u64),
        Str(String),
    }

    match Option::<NumOrStr>::deserialize(deserializer)? {
        Some(NumOrStr::Num(n)) => Ok(Some(n)),
        Some(NumOrStr::Str(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed.parse::<u64>().map(Some).map_err(de::Error::custom)
            }
        }
        None => Ok(None),
    }
}

/// Metadata object from the `messages` array.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Message {
    /// `"ok"` on success, or an error description.
    #[serde(default)]
    pub status: String,

    /// Date interval echoed back (e.g. `"2024-01-01:2024-01-15"`).
    #[serde(default)]
    pub interval: Option<String>,

    /// Current cursor position.
    #[serde(default, deserialize_with = "deserialize_optional_u64_loose")]
    pub cursor: Option<u64>,

    /// Number of items returned in this page.
    #[serde(default, deserialize_with = "deserialize_optional_u64_loose")]
    pub count: Option<u64>,

    /// Number of *new* papers in this interval (details endpoint only).
    #[serde(default, deserialize_with = "deserialize_optional_u64_loose")]
    pub count_new_papers: Option<u64>,

    /// Total items matching the query across all pages.
    #[serde(default, deserialize_with = "deserialize_optional_u64_loose")]
    pub total: Option<u64>,
}

impl Message {
    /// `true` if the API reported success.
    pub fn is_ok(&self) -> bool {
        self.status.eq_ignore_ascii_case("ok")
    }
}

// ---------------------------------------------------------------------------
// Details endpoint — single preprint record
// ---------------------------------------------------------------------------

/// A single preprint record from the `/details` endpoint.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BiorxivEntry {
    /// DOI of the preprint (e.g. `"10.1101/2024.01.15.573421"`).
    #[serde(default)]
    pub doi: String,

    /// Manuscript title.
    #[serde(default)]
    pub title: String,

    /// Authors as a semicolon-separated string,
    /// e.g. `"Watson, O. J.; Tran, T. N.-A.; Zupko, R. J."`.
    #[serde(default)]
    pub authors: String,

    /// Corresponding author full name.
    #[serde(default)]
    pub author_corresponding: Option<String>,

    /// Corresponding author institution.
    #[serde(default)]
    pub author_corresponding_institution: Option<String>,

    /// Posting date (`YYYY-MM-DD`).
    #[serde(default)]
    pub date: String,

    /// Version number as a string (e.g. `"1"`, `"2"`).
    #[serde(default)]
    pub version: String,

    /// Manuscript type (e.g. `"new_result"`, `"PUBLISHAHEADOFPRINT"`).
    #[serde(default, rename = "type")]
    pub article_type: String,

    /// License key (e.g. `"cc_by"`, `"cc_by_nc_nd"`, `"cc_no"`).
    #[serde(default)]
    pub license: String,

    /// Subject category (e.g. `"infectious diseases"`, `"genetics"`).
    #[serde(default)]
    pub category: String,

    /// URL to the JATS XML source.
    #[serde(default)]
    pub jatsxml: String,

    /// Abstract text.
    #[serde(default)]
    pub abstract_text: String,

    /// DOI of the published (journal) version, or `"NA"`.
    #[serde(default)]
    pub published: String,

    /// `"medrxiv"`, `"medRxiv"`, `"biorxiv"`, or `"bioRxiv"`.
    #[serde(default)]
    pub server: String,

    /// Funding information, or `"NA"`.
    #[serde(default)]
    pub funder: String,
}

/// Type alias for the common details response.
pub type DetailsResponse = ApiResponse<BiorxivEntry>;

// ---------------------------------------------------------------------------
// Pub endpoint — preprint → published mapping
// ---------------------------------------------------------------------------

/// A single mapping from a preprint DOI to its published journal version.
///
/// Returned by the `/pub` endpoint (only available on `api.biorxiv.org`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PubMapping {
    /// Preprint DOI.
    #[serde(default)]
    pub biorxiv_doi: String,

    /// DOI of the published journal article.
    #[serde(default)]
    pub published_doi: String,

    /// Preprint title.
    #[serde(default)]
    pub preprint_title: String,

    /// Preprint subject category.
    #[serde(default)]
    pub preprint_category: String,

    /// Preprint posting date (`YYYY-MM-DD`).
    #[serde(default)]
    pub preprint_date: String,

    /// Published-article date (`YYYY-MM-DD`).
    #[serde(default)]
    pub published_date: String,
}

/// Type alias for the pub response.
pub type PubResponse = ApiResponse<PubMapping>;
