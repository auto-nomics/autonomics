//! Error and result types for the visualization store.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0} must not be blank")]
    Invalid(&'static str),
    #[error("project or visualization not found")]
    NotFound,
    #[error("document changed; reload before saving")]
    Conflict,
}

pub type Result<T> = std::result::Result<T, Error>;
