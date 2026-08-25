use thiserror::Error;

use agentik_core::error::AgentError;
use agentik_core::storage::StorageError;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised while opening or operating the runtime host.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("failed to build agent: {0}")]
    AgentBuild(#[from] AgentError),

    #[error("{0}")]
    Engine(#[from] data_engine::error::Error),

    #[error("OpenGWAS setup failed: {0}")]
    Opengwas(#[from] opengwas::OpengwasError),

    #[error("agent storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("bibliography init failed: {0}")]
    Bib(#[from] bib_base::Error),

    #[error("writing system init failed: {0}")]
    Writing(#[from] writing_base::Error),

    #[error("{0}")]
    Other(String),
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}
