pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Custom(String),

    #[error("Datalake error")]
    DatalakeError(#[from] datalake::error::Error),

    #[error("Iceberg datalake is missing")]
    MissDatalake,

    #[error(transparent)]
    Dag(#[from] crate::dag::DagError),

    #[error(transparent)]
    NodeRegistry(#[from] crate::node_registry::error::Error),
}
