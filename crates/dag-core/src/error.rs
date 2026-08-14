pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Custom(String),

    #[error(transparent)]
    Dag(#[from] crate::dag::DagError),

    #[error(transparent)]
    NodeRegistry(#[from] crate::registry::error::Error),
}
