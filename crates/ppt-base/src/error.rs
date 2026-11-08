//! Error type for the deck engine.

/// Result alias for deck-engine operations.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("deck {0} not found")]
    DeckNotFound(String),

    #[error("deck {deck} has {total} slides (1-{total}); got slide {no}")]
    SlideOutOfRange {
        deck: String,
        no: usize,
        total: usize,
    },

    #[error("invalid frontmatter YAML: {0}")]
    BadFrontmatter(String),

    #[error("slide 1 frontmatter is the deck headmatter and cannot be replaced here; patch the deck headmatter instead")]
    HeadmatterGuard,

    #[error("invalid theme CSS: {0}")]
    InvalidThemeCss(String),

    #[error("invalid aspect ratio: {0}")]
    BadAspectRatio(#[from] ppt_types::ParseAspectError),

    #[error("invalid page range: {0}")]
    BadRange(#[from] ppt_types::ParseRangeError),

    #[error("revision conflict: deck is at {expected}, request was based on {actual}")]
    RevisionConflict { expected: String, actual: String },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("export error: {0}")]
    Export(String),

    #[error("asset error: {0}")]
    Asset(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl From<turso::Error> for Error {
    fn from(e: turso::Error) -> Self {
        Error::Storage(e.to_string())
    }
}
