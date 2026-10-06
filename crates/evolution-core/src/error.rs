//! Errors returned by shared evolution stores and routers.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read `{path}`: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot write `{path}`: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot serialize `{path}`: {source}")]
    Serialize {
        path: PathBuf,
        source: toml::ser::Error,
    },

    #[error("cannot parse `{path}`: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
