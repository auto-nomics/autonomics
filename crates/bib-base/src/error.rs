use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unknown error: {0}")]
    Unknown(String),
    #[error("{0}")]
    Turso(#[from] turso::Error),
    #[error("search index: {0}")]
    Index(#[from] tantivy::TantivyError),
    #[error("search index I/O: {0}")]
    IndexIo(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid collection: {0}")]
    InvalidCollection(String),
    #[error("invalid search query: {0}")]
    InvalidSearchQuery(String),
    #[error("search request cancelled")]
    SearchCancelled,
    #[error("semantic search unavailable: {0}")]
    SemanticUnavailable(String),
    #[error(
        "semantic resource limit: {resource} requires {used}, configured maximum is {maximum}; increase the budget or reduce indexed content"
    )]
    SemanticResourceLimit {
        resource: String,
        used: u64,
        maximum: u64,
    },
    #[error("search conflict: {0}")]
    SearchConflict(String),
    #[error("collection conflict: {0}")]
    CollectionConflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(
        "bib-base storage actor stopped (all handles dropped or the actor task panicked); \
         re-open the database for a fresh handle"
    )]
    ActorStopped,
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.into())
    }
}
