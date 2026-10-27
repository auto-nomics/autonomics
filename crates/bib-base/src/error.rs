use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unknown error: {0}")]
    Unknown(String),
    #[error("{0}")]
    Turso(#[from] turso::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid collection: {0}")]
    InvalidCollection(String),
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
