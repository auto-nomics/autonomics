//! Serde types for the Crossref REST API JSON responses.
//!
//! Crossref wraps every response in an envelope:
//!
//! ```json
//! { "status": "ok", "message-type": "work", "message": { ... } }
//! ```
//!
//! For list responses `message` contains `total-results`, `items-per-page`,
//! `query` echo, `next-cursor`, and an `items` array.
//!
//! The Crossref work schema is very rich and varies across records. We model
//! the common fields with `#[serde(default)]` so that missing or null fields
//! degrade gracefully. Unknown fields are silently ignored.

use serde::{Deserialize, Deserializer, Serialize};

// ---------------------------------------------------------------------------
// Null-tolerant deserialization helpers
// ---------------------------------------------------------------------------

/// Deserialize a `T` that implements `Default`, treating JSON `null` as the
/// default value. This is needed because Crossref sometimes returns `null`
/// for numeric fields (e.g. `is-referenced-by-count: null`).
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

// ===========================================================================
// Envelope wrappers
// ===========================================================================

/// Top-level envelope for singleton responses (single work, funder, etc.).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEnvelope<T> {
    #[serde(default)]
    pub status: String,
    #[serde(rename = "message-type", default)]
    pub message_type: String,
    #[serde(rename = "message-version", default)]
    pub message_version: String,
    pub message: Option<T>,
}

/// Top-level envelope for list responses.
#[derive(Debug, Clone, Deserialize)]
pub struct ListEnvelope<T> {
    #[serde(default)]
    pub status: String,
    #[serde(rename = "message-type", default)]
    pub message_type: String,
    #[serde(rename = "message-version", default)]
    pub message_version: String,
    #[serde(default)]
    pub message: ListMessage<T>,
}

/// The `message` object inside a list envelope.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListMessage<T> {
    #[serde(default, rename = "total-results", deserialize_with = "null_default")]
    pub total_results: u64,
    #[serde(default, rename = "items-per-page")]
    pub items_per_page: Option<u32>,
    #[serde(default, rename = "query")]
    pub query: serde_json::Value,
    #[serde(default, rename = "next-cursor")]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub items: Vec<T>,
}

// ===========================================================================
// Work — the central entity
// ===========================================================================

/// A single Crossref metadata record (journal article, book, preprint, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Work {
    /// The DOI of this work.
    #[serde(default, rename = "DOI")]
    pub doi: String,

    /// Title(s). Typically one entry; may include translated titles.
    #[serde(default)]
    pub title: Vec<String>,

    /// Subtitle(s), if present.
    #[serde(default)]
    pub subtitle: Vec<String>,

    /// Container title (journal name, book title, …).
    #[serde(default, rename = "container-title")]
    pub container_title: Vec<String>,

    /// Short container title (journal abbreviation).
    #[serde(default, rename = "short-container-title")]
    pub short_container_title: Vec<String>,

    /// Authors / creators.
    #[serde(default)]
    pub author: Vec<Author>,

    /// Published-online date parts.
    #[serde(default, rename = "published-online")]
    pub published_online: Option<DateParts>,

    /// Published-print date parts.
    #[serde(default, rename = "published-print")]
    pub published_print: Option<DateParts>,

    /// Issued date (earliest known publication date).
    #[serde(default)]
    pub issued: Option<DateParts>,

    /// Created date.
    #[serde(default)]
    pub created: Option<DateParts>,

    /// Deposited date.
    #[serde(default)]
    pub deposited: Option<DateParts>,

    /// Abstract (may contain JATS XML tags).
    #[serde(default)]
    pub abstract_text: Option<String>,

    /// Publisher name.
    #[serde(default)]
    pub publisher: String,

    /// Volume.
    #[serde(default)]
    pub volume: String,

    /// Issue.
    #[serde(default)]
    pub issue: String,

    /// Page range.
    #[serde(default)]
    pub page: String,

    /// Article number (e.g. e-locators).
    #[serde(default, rename = "article-number")]
    pub article_number: String,

    /// ISSN list (print and/or electronic).
    #[serde(default)]
    pub issn: Vec<String>,

    /// ISBN list.
    #[serde(default)]
    pub isbn: Vec<String>,

    /// Work type (e.g. `journal-article`, `book-chapter`, `posted-content`).
    #[serde(default)]
    pub r#type: String,

    /// Number of times this DOI is referenced by other Crossref DOIs.
    #[serde(default, rename = "is-referenced-by-count", deserialize_with = "null_default")]
    pub is_referenced_by_count: u64,

    /// Number of references in this work's reference list.
    #[serde(default, rename = "references-count", deserialize_with = "null_default")]
    pub references_count: u64,

    /// References list (when included — requires `has-references` filter).
    #[serde(default)]
    pub reference: Vec<Reference>,

    /// License information.
    #[serde(default)]
    pub license: Vec<License>,

    /// Funder information.
    #[serde(default)]
    pub funder: Vec<Funder>,

    /// Link to full text resources.
    #[serde(default)]
    pub link: Vec<Link>,

    /// Subject categories.
    #[serde(default)]
    pub subject: Vec<String>,

    /// Language.
    #[serde(default)]
    pub language: String,

    /// Alternative IDs assigned by the publisher.
    #[serde(default, rename = "alternative-id")]
    pub alternative_id: Vec<String>,

    /// Publisher prefix (DOI owner prefix).
    #[serde(default)]
    pub prefix: String,

    /// Crossref member ID of the depositor.
    #[serde(default)]
    pub member: String,

    /// Score (relevance) returned by the search engine.
    #[serde(default)]
    pub score: Option<f64>,

    /// URL for the DOI.
    #[serde(default, rename = "URL")]
    pub url: String,
}

impl Work {
    /// First title, or a placeholder.
    pub fn title_str(&self) -> &str {
        self.title.first().map(|s| s.as_str()).unwrap_or("(untitled)")
    }

    /// First container title (journal / book name).
    pub fn journal_str(&self) -> Option<&str> {
        self.container_title.first().map(|s| s.as_str())
    }

    /// Best publication year from issued / published-online / published-print.
    pub fn year(&self) -> Option<u16> {
        self.issued
            .as_ref()
            .or(self.published_online.as_ref())
            .or(self.published_print.as_ref())
            .and_then(|d| d.year())
    }
}

// ===========================================================================
// Author
// ===========================================================================

/// An author on a Crossref work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub given: String,
    #[serde(default)]
    pub family: String,
    /// `author` (writer), `editor`, `chair`, `translator`, …
    #[serde(default)]
    pub sequence: String,
    /// ORCID URL if available.
    #[serde(default, rename = "ORCID")]
    pub orcid: Option<String>,
    /// Affiliation list.
    #[serde(default)]
    pub affiliation: Vec<Affiliation>,
}

impl Author {
    /// Display name: `"Family Given"` or just one part.
    pub fn display(&self) -> String {
        match (&self.family, &self.given) {
            (f, g) if !f.is_empty() && !g.is_empty() => format!("{f} {g}"),
            (f, _) if !f.is_empty() => f.clone(),
            (_, g) if !g.is_empty() => g.clone(),
            _ => String::new(),
        }
    }
}

/// Affiliation entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Affiliation {
    #[serde(default)]
    pub name: String,
}

// ===========================================================================
// Date — Crossref uses nested "date-parts" arrays
// ===========================================================================

/// Crossref date object with nested `date-parts`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DateParts {
    #[serde(default, rename = "date-parts")]
    pub date_parts: Vec<Vec<Option<u32>>>,
    #[serde(default, rename = "date-time")]
    pub date_time: Option<String>,
    /// The ISO timestamp when this date was first deposited (also used inside
    /// `created` / `deposited` objects).
    #[serde(default, rename = "timestamp")]
    pub timestamp: Option<u64>,
}

impl DateParts {
    /// Extract the year from the first date-parts entry.
    pub fn year(&self) -> Option<u16> {
        self.date_parts
            .first()
            .and_then(|parts| parts.first())
            .copied()
            .flatten()
            .map(|y| y as u16)
    }

    /// Extract `(year, month, day)` from the first date-parts entry.
    pub fn ymd(&self) -> (Option<u16>, Option<u8>, Option<u8>) {
        let parts = self.date_parts.first();
        let y = parts.and_then(|p| p.first()).copied().flatten().map(|v| v as u16);
        let m = parts.and_then(|p| p.get(1)).copied().flatten().map(|v| v as u8);
        let d = parts.and_then(|p| p.get(2)).copied().flatten().map(|v| v as u8);
        (y, m, d)
    }
}

// ===========================================================================
// Reference, License, Funder, Link
// ===========================================================================

/// A reference in a work's reference list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reference {
    /// DOI of the referenced work (when available).
    #[serde(default, rename = "DOI")]
    pub doi: Option<String>,
    /// Article title from the reference.
    #[serde(default, rename = "article-title")]
    pub article_title: Option<String>,
    /// Author string.
    #[serde(default)]
    pub author: Option<String>,
    /// Journal / container title.
    #[serde(default, rename = "journal-title")]
    pub journal_title: Option<String>,
    /// Year.
    #[serde(default)]
    pub year: Option<String>,
    /// Volume.
    #[serde(default)]
    pub volume: Option<String>,
    /// Issue.
    #[serde(default)]
    pub issue: Option<String>,
    /// First page.
    #[serde(default, rename = "first-page")]
    pub first_page: Option<String>,
    /// Unstructured citation text.
    #[serde(default)]
    pub unstructured: Option<String>,
    /// Key used by the source article.
    #[serde(default)]
    pub key: String,
}

/// License attached to a work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct License {
    #[serde(default, rename = "URL")]
    pub url: String,
    #[serde(default)]
    pub start: Option<DateParts>,
    #[serde(default, rename = "content-version")]
    pub content_version: String,
    #[serde(default, rename = "delay")]
    pub delay: Option<u64>,
}

/// Funder entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Funder {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "DOI")]
    pub doi: Option<String>,
    #[serde(default)]
    pub award: Vec<String>,
    #[serde(default, rename = "doi-asserted-by")]
    pub doi_asserted_by: Option<String>,
}

/// Full text resource link.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "content-type")]
    pub content_type: String,
    #[serde(default, rename = "content-version")]
    pub content_version: String,
    #[serde(default, rename = "intended-application")]
    pub intended_application: String,
}

// ===========================================================================
// Other resource components (members, journals, funders, types, prefixes, licenses)
// ===========================================================================

/// A Crossref member (publisher / registering organisation).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Member {
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    #[serde(default, rename = "primary-name")]
    pub primary_name: String,
    #[serde(default, rename = "names")]
    pub names: serde_json::Value,
    #[serde(default)]
    pub prefix: serde_json::Value,
    #[serde(default, rename = "backfile-doi-count", deserialize_with = "null_default")]
    pub backfile_doi_count: u64,
    #[serde(default, rename = "current-doi-count", deserialize_with = "null_default")]
    pub current_doi_count: u64,
    #[serde(default, rename = "total-doi-count", deserialize_with = "null_default")]
    pub total_doi_count: u64,
    #[serde(default, rename = "coverage-type")]
    pub coverage_type: serde_json::Value,
    #[serde(default)]
    pub counts: serde_json::Value,
    #[serde(default)]
    pub breakdowns: serde_json::Value,
    #[serde(default)]
    pub location: serde_json::Value,
    #[serde(default)]
    pub country: Option<String>,
}

/// A journal record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub issn: Vec<String>,
    #[serde(default)]
    pub publisher: String,
    #[serde(default, rename = "last-status-check-time")]
    pub last_status_check_time: Option<u64>,
    #[serde(default)]
    pub counts: serde_json::Value,
    #[serde(default, rename = "coverage-type")]
    pub coverage_type: serde_json::Value,
    #[serde(default, rename = "breakdowns")]
    pub breakdowns: serde_json::Value,
}

/// A funder record (from the Open Funder Registry).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunderRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "uri")]
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "alt-names")]
    pub alt_names: serde_json::Value,
    #[serde(default)]
    pub location: String,
    #[serde(default, rename = "canonical")]
    pub canonical: Option<String>,
    #[serde(default, rename = "replacement")]
    pub replacement: Vec<String>,
    #[serde(default)]
    pub counts: serde_json::Value,
    #[serde(default, rename = "hierarchy-names")]
    pub hierarchy_names: serde_json::Value,
    #[serde(default)]
    pub tokens: Vec<String>,
}

/// A work type (e.g. `journal-article`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkType {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
}

/// A license record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LicenseRecord {
    #[serde(default, rename = "URL")]
    pub url: String,
    #[serde(default, rename = "work-count")]
    pub work_count: Option<u64>,
}

// ===========================================================================
// Agency response (/works/{doi}/agency)
// ===========================================================================

/// DOI registration agency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgencyInfo {
    #[serde(default, rename = "DOI")]
    pub doi: String,
    #[serde(default)]
    pub agency: AgencyDetail,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgencyDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
}

// ===========================================================================
// Convenience type aliases
// ===========================================================================

pub type WorkResponse = MessageEnvelope<Work>;
pub type WorksListResponse = ListEnvelope<Work>;
pub type MemberResponse = MessageEnvelope<Member>;
pub type MembersListResponse = ListEnvelope<Member>;
pub type JournalResponse = MessageEnvelope<Journal>;
pub type JournalsListResponse = ListEnvelope<Journal>;
pub type FunderResponse = MessageEnvelope<FunderRecord>;
pub type FundersListResponse = ListEnvelope<FunderRecord>;
pub type TypesListResponse = ListEnvelope<WorkType>;
pub type LicensesListResponse = ListEnvelope<LicenseRecord>;
pub type AgencyResponse = MessageEnvelope<AgencyInfo>;
