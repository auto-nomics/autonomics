//! Core data model for the bibliography management library.
//!
//! All structs are plain serializable data — no I/O, no side-effects.
//! They are designed to map cleanly onto both structured storage rows and
//! CSL-JSON / BibTeX export formats.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Article — the central entity
// ---------------------------------------------------------------------------

/// A single bibliographic record (journal article, preprint, book chapter, …).
///
/// The struct is normalized: every external identifier lives in
/// [`Article::identifiers`] so that dedup logic can match on any key
/// (DOI, PMID, arXiv, …) without special-casing fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    /// Internal unique identifier (UUID or opaque string).
    pub id: String,

    /// Canonical title.
    pub title: String,

    /// Structured author list (order preserved).
    #[serde(default)]
    pub authors: Vec<Author>,

    /// External identifiers — the primary keys used for dedup.
    ///
    /// At least one entry is recommended. DOI has the highest priority
    /// for dedup, followed by PMID, then arXiv.
    #[serde(default)]
    pub identifiers: Vec<Identifier>,

    /// Abstract text (plain text, no markup).
    #[serde(default)]
    pub abstract_text: Option<String>,

    /// Publication year (e.g. `2024`).
    #[serde(default)]
    pub year: Option<u16>,

    /// Publication month (1–12) if known.
    #[serde(default)]
    pub month: Option<u8>,

    /// Journal or venue name.
    #[serde(default)]
    pub journal: Option<String>,

    /// Journal volume.
    #[serde(default)]
    pub volume: Option<String>,

    /// Issue number.
    #[serde(default)]
    pub issue: Option<String>,

    /// Page range (e.g. `"1-15"` or `"e012345"`).
    #[serde(default)]
    pub pages: Option<String>,

    /// Journal ISSN (print), used for dedup and BibTeX export.
    #[serde(default)]
    pub issn: Option<String>,

    /// Journal EISSN (electronic), if distinct from ISSN.
    #[serde(default)]
    pub essn: Option<String>,

    /// Article language (e.g. `"eng"`, `"chi"`), ISO 639-3 where available.
    #[serde(default)]
    pub language: Option<String>,

    /// Publication types from the source (e.g. `"Journal Article"`,
    /// `"Review"`, `"Preprint"`).
    #[serde(default)]
    pub pub_types: Vec<String>,

    /// Controlled-vocabulary keywords (MeSH terms, author keywords).
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Where this record was obtained from.
    #[serde(default)]
    pub source: ArticleSource,

    /// Timestamps.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Article {
    /// Convenience constructor with just an ID and title.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            title: title.into(),
            authors: Vec::new(),
            identifiers: Vec::new(),
            abstract_text: None,
            year: None,
            month: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            issn: None,
            essn: None,
            language: None,
            pub_types: Vec::new(),
            keywords: Vec::new(),
            source: ArticleSource::default(),
            created_at: Some(now),
            updated_at: Some(now),
        }
    }

    /// Return the first identifier of the given kind, if any.
    pub fn identifier(&self, kind: IdKind) -> Option<&str> {
        self.identifiers
            .iter()
            .find(|i| i.kind == kind)
            .map(|i| i.value.as_str())
    }

    /// Return the DOI if present (the most common dedup key).
    pub fn doi(&self) -> Option<&str> {
        self.identifier(IdKind::Doi)
    }

    /// Return the PMID if present.
    pub fn pmid(&self) -> Option<&str> {
        self.identifier(IdKind::Pmid)
    }

    /// Return a formatted citation string in a compact author-year style
    /// (e.g. `"Smith et al., 2024"`).  Useful for quick display.
    pub fn short_cite(&self) -> String {
        let author_part = match self.authors.first() {
            Some(a) => {
                let n = self.authors.len();
                if n > 1 {
                    format!("{} et al.", a.last_name)
                } else {
                    a.last_name.clone()
                }
            }
            None => "Anonymous".to_string(),
        };
        match self.year {
            Some(y) => format!("{author_part}, {y}"),
            None => format!("{author_part}, n.d."),
        }
    }

    // --- builder helpers ---

    /// Attach a DOI to this article.
    pub fn with_doi(mut self, doi: impl Into<String>) -> Self {
        self.identifiers.push(Identifier::doi(doi));
        self
    }

    /// Attach a PMID to this article.
    pub fn with_pmid(mut self, pmid: impl Into<String>) -> Self {
        self.identifiers.push(Identifier::pmid(pmid));
        self
    }
}

// ---------------------------------------------------------------------------
// Author
// ---------------------------------------------------------------------------

/// A single author / contributor on an article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// Family / last name.
    pub last_name: String,

    /// Given / first name(s).
    #[serde(default)]
    pub fore_name: Option<String>,

    /// Initials (e.g. `"JQ"`), if provided by the source.
    #[serde(default)]
    pub initials: Option<String>,

    /// Institutional affiliation string.
    #[serde(default)]
    pub affiliation: Option<String>,

    /// ORCID iD without URL prefix (e.g. `"0000-0002-1825-0097"`).
    #[serde(default)]
    pub orcid: Option<String>,

    /// `true` if this author is a corresponding author.
    #[serde(default)]
    pub corresponding: bool,
}

impl Author {
    /// Build a display name, e.g. `"Smith J"`.
    pub fn display_name(&self) -> String {
        match (&self.fore_name, &self.initials) {
            (Some(f), _) => format!("{} {}", self.last_name, f),
            (None, Some(i)) => format!("{} {}", self.last_name, i),
            (None, None) => self.last_name.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Identifier
// ---------------------------------------------------------------------------

/// The category of an external identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdKind {
    /// Digital Object Identifier — highest-priority dedup key.
    Doi,
    /// PubMed unique identifier.
    Pmid,
    /// PubMed Central identifier.
    Pmc,
    /// Embase identifier (accession number / LUI).
    Embase,
    /// arXiv preprint identifier.
    Arxiv,
    /// bioRxiv / medRxiv DOI (same DOI namespace, no separate ID).
    Biorxiv,
    /// Semantic Scholar paper ID.
    S2,
    /// OpenAlex work ID (e.g. `"W1234567890"`).
    OpenAlex,
    /// Any other identifier (e.g. ISBN, handle).
    Other,
}

impl IdKind {
    /// Lowercased string label used for serde and column naming.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Doi => "doi",
            Self::Pmid => "pmid",
            Self::Pmc => "pmc",
            Self::Embase => "embase",
            Self::Arxiv => "arxiv",
            Self::Biorxiv => "biorxiv",
            Self::S2 => "s2",
            Self::OpenAlex => "openalex",
            Self::Other => "other",
        }
    }
}

/// A single external identifier attached to an article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifier {
    /// What kind of identifier this is.
    pub kind: IdKind,
    /// The identifier value (normalised — no URL prefix).
    pub value: String,
}

impl Identifier {
    pub fn new(kind: IdKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }

    /// Convenience constructor for a DOI.
    pub fn doi(value: impl Into<String>) -> Self {
        Self::new(IdKind::Doi, value)
    }

    /// Convenience constructor for a PMID.
    pub fn pmid(value: impl Into<String>) -> Self {
        Self::new(IdKind::Pmid, value)
    }
}

// ---------------------------------------------------------------------------
// ArticleSource — provenance
// ---------------------------------------------------------------------------

/// Where a record entered the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArticleSource {
    /// Imported from NCBI PubMed via E-utilities.
    Pubmed,
    /// Imported from Embase via the Elsevier Embase API.
    Embase,
    /// Fetched directly from a DOI resolver (CrossRef, DataCite).
    CrossRef,
    /// Imported from arXiv.
    Arxiv,
    /// Imported from bioRxiv or medRxiv.
    Biorxiv,
    /// Imported from Europe PMC.
    EuropePmc,
    /// Imported from Semantic Scholar.
    SemanticScholar,
    /// Imported from the GWAS Catalog (EBI).
    GwasCatalog,
    /// Imported from OpenAlex.
    OpenAlex,
    /// Manually entered by the user.
    Manual,
    /// Synced from an external reference manager (e.g. Zotero).
    Zotero,
    /// Unknown or unspecified origin.
    #[default]
    Unknown,
}

// ---------------------------------------------------------------------------
// Annotation
// ---------------------------------------------------------------------------

/// The kind of user-added annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    /// A free-text note attached to the article.
    Note,
    /// A highlighted excerpt from the text.
    Highlight,
    /// A comment or critique.
    Comment,
}

impl AnnotationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Highlight => "highlight",
            Self::Comment => "comment",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "highlight" => Self::Highlight,
            "comment" => Self::Comment,
            _ => Self::Note,
        }
    }
}

/// A user-created note, highlight, or comment on an article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    /// Internal annotation ID.
    pub id: String,

    /// What kind of annotation this is.
    pub kind: AnnotationKind,

    /// The annotation body — note text, highlighted excerpt, or comment.
    pub content: String,

    /// PDF page number (1-based), if the annotation is anchored to a page.
    #[serde(default)]
    pub page: Option<u32>,

    /// Creation timestamp.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

impl Annotation {
    pub fn new(id: impl Into<String>, kind: AnnotationKind, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            content: content.into(),
            page: None,
            created_at: Some(Utc::now()),
        }
    }
}

// ---------------------------------------------------------------------------
// Collection status
// ---------------------------------------------------------------------------

/// Lifecycle status of a collection (research investigation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CollectionStatus {
    /// Actively being worked on by an agent.
    #[default]
    Active,
    /// Investigation finished; collection is frozen.
    Completed,
    /// No longer relevant; kept for history.
    Archived,
}

impl CollectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Archived => "archived",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "archived" => Self::Archived,
            _ => Self::Active,
        }
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// A named, ordered group of articles — a reading list, research
/// investigation, or project bibliography.
///
/// Articles are linked to a collection via [`CollectionArticle`], which
/// carries per-article semantic metadata (role, fetch status, …).
/// The [`Collection::article_ids`] field is a convenience hydration of
/// the ordered article IDs, populated on load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Internal collection ID.
    pub id: String,

    /// Human-readable name (e.g. `"MR eQTL references"`).
    pub name: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,

    /// IDs of articles in this collection (order preserved).
    #[serde(default)]
    pub article_ids: Vec<String>,

    /// Free-form tags on the collection itself.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Lifecycle status.
    #[serde(default)]
    pub status: CollectionStatus,

    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Collection {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            article_ids: Vec::new(),
            tags: Vec::new(),
            status: CollectionStatus::Active,
            created_at: Some(now),
            updated_at: Some(now),
        }
    }
}

// ---------------------------------------------------------------------------
// Collection-article association
// ---------------------------------------------------------------------------

/// Why an article belongs to a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArticleRole {
    /// Agent actively requested this article for the investigation.
    Requested,
    /// Referenced as supporting context.
    #[default]
    Referenced,
    /// Cited in the collection's output (report, manuscript, …).
    Cited,
    /// General background knowledge.
    Background,
}

impl ArticleRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Referenced => "referenced",
            Self::Cited => "cited",
            Self::Background => "background",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "requested" => Self::Requested,
            "cited" => Self::Cited,
            "background" => Self::Background,
            _ => Self::Referenced,
        }
    }
}

/// Full-text acquisition lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    /// Only metadata is available.
    #[default]
    MetadataOnly,
    /// Agent needs the full text; waiting for a user upload.
    FulltextRequested,
    /// Full text is available (user-uploaded or open-access).
    FulltextAvailable,
}

impl FetchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetadataOnly => "metadata_only",
            Self::FulltextRequested => "fulltext_requested",
            Self::FulltextAvailable => "fulltext_available",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "fulltext_requested" => Self::FulltextRequested,
            "fulltext_available" => Self::FulltextAvailable,
            _ => Self::MetadataOnly,
        }
    }
}

/// Who added an article to a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AddedBy {
    #[default]
    Agent,
    User,
}

impl AddedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::User => "user",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "user" => Self::User,
            _ => Self::Agent,
        }
    }
}

/// The association between a collection and an article, enriched with
/// semantic metadata that drives the agent workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionArticle {
    pub collection_id: String,
    pub article_id: String,
    /// Ordering within the collection (0-based).
    pub position: i32,
    /// Why this article is in the collection.
    pub role: ArticleRole,
    /// Full-text acquisition state.
    pub fetch_status: FetchStatus,
    /// Whether the agent or a user added it.
    pub added_by: AddedBy,
    /// Agent-supplied note explaining the article's relevance.
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub added_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Full text
// ---------------------------------------------------------------------------

/// File format of a stored full text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Pdf,
    Html,
    Txt,
}

impl FileFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Txt => "txt",
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().trim_start_matches('.') {
            "pdf" => Self::Pdf,
            "html" | "htm" => Self::Html,
            _ => Self::Txt,
        }
    }
}

/// Where a stored full text originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullTextSource {
    /// User manually uploaded the file.
    UserUpload,
    /// Automatically downloaded from an open-access source.
    OpenAccess,
}

impl FullTextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserUpload => "user_upload",
            Self::OpenAccess => "open_access",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "open_access" => Self::OpenAccess,
            _ => Self::UserUpload,
        }
    }
}

/// Full text stored alongside an article.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullText {
    /// Article this full text belongs to.
    pub article_id: String,

    /// Path within the file store (e.g. `"/articles/abc123.pdf"`).
    pub file_path: String,

    /// Format of the stored file.
    pub file_format: FileFormat,

    /// Extracted plain-text content (used for FTS indexing and retrieval).
    #[serde(default)]
    pub text_content: Option<String>,

    /// Provenance of the file.
    pub source: FullTextSource,

    /// SHA-256 hash for deduplication.
    #[serde(default)]
    pub file_hash: Option<String>,

    /// File size in bytes.
    #[serde(default)]
    pub file_size: Option<i64>,

    #[serde(default)]
    pub uploaded_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// SearchHit — FTS result
// ---------------------------------------------------------------------------

/// A single full-text search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub article_id: String,
    pub title: String,
    /// BM25 relevance score (lower = more relevant in SQLite FTS5).
    pub score: f64,
    /// Text snippet around the best matching position.
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// Reference — citation edge
// ---------------------------------------------------------------------------

/// A directed citation edge: `source` cites `target`.
///
/// Both endpoints reference article IDs. This struct is deliberately minimal
/// so that a citation graph is just `Vec<Reference>` and can be fed into
/// graph algorithms or a DataFusion edge table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// ID of the citing article.
    pub source: String,

    /// ID of the cited article.
    pub target: String,
}

// ---------------------------------------------------------------------------
// ExportFormat — supported output formats
// ---------------------------------------------------------------------------

/// The bibliographic export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// BibTeX (`.bib`).
    Bibtex,
    /// RIS (Refman / EndNote import).
    Ris,
    /// CSL-JSON (Citation Style Language).
    CslJson,
    /// Markdown with a numbered reference list.
    Markdown,
}

impl ExportFormat {
    /// File extension commonly associated with this format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Bibtex => "bib",
            Self::Ris => "ris",
            Self::CslJson => "json",
            Self::Markdown => "md",
        }
    }
}

// ---------------------------------------------------------------------------
// LibraryEntry — article + its attached annotations and tags
// ---------------------------------------------------------------------------

/// A fully hydrated library record: the article plus user-managed data
/// that is stored alongside it but not part of the bibliographic metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEntry {
    /// The bibliographic record.
    pub article: Article,

    /// User annotations on this article.
    #[serde(default)]
    pub annotations: Vec<Annotation>,

    /// User-assigned tags (distinct from source keywords).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Path or URI of the associated PDF within the file store, if any.
    #[serde(default)]
    pub pdf_path: Option<String>,

    /// Collections this article belongs to (collection IDs).
    #[serde(default)]
    pub collections: Vec<String>,
}

impl LibraryEntry {
    /// Wrap an article into a fresh library entry with no annotations.
    pub fn from_article(article: Article) -> Self {
        Self {
            article,
            annotations: Vec::new(),
            tags: Vec::new(),
            pdf_path: None,
            collections: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_new_sets_defaults() {
        let art = Article::new("a1", "On the origin of species");
        assert_eq!(art.title, "On the origin of species");
        assert!(art.identifiers.is_empty());
        assert!(art.doi().is_none());
        assert!(art.created_at.is_some());
    }

    #[test]
    fn identifier_helpers() {
        let art = Article::new("a1", "Test")
            .with_doi("10.1000/test")
            .with_pmid("12345");
        assert_eq!(art.doi(), Some("10.1000/test"));
        assert_eq!(art.pmid(), Some("12345"));
        assert_eq!(art.identifier(IdKind::Pmc), None);
    }

    #[test]
    fn short_cite_single_author() {
        let mut art = Article::new("a1", "Foo");
        art.authors.push(Author {
            last_name: "Darwin".into(),
            fore_name: None,
            initials: Some("C".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        art.year = Some(1859);
        assert_eq!(art.short_cite(), "Darwin, 1859");
    }

    #[test]
    fn short_cite_multi_author() {
        let mut art = Article::new("a1", "Foo");
        art.authors.push(Author {
            last_name: "Smith".into(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        art.authors.push(Author {
            last_name: "Jones".into(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        assert_eq!(art.short_cite(), "Smith et al., n.d.");
    }

    #[test]
    fn author_display_name() {
        let a = Author {
            last_name: "Smith".into(),
            fore_name: Some("John".into()),
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        };
        assert_eq!(a.display_name(), "Smith John");
    }

    #[test]
    fn export_format_extension() {
        assert_eq!(ExportFormat::Bibtex.extension(), "bib");
        assert_eq!(ExportFormat::CslJson.extension(), "json");
    }

    #[test]
    fn library_entry_from_article() {
        let art = Article::new("a1", "Test");
        let entry = LibraryEntry::from_article(art);
        assert!(entry.annotations.is_empty());
        assert!(entry.tags.is_empty());
        assert!(entry.pdf_path.is_none());
    }
}
