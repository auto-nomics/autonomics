use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid plugin name `{name}`: {reason}")]
    InvalidPluginName { name: String, reason: String },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("invalid plugin transition {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("plugin registry operation failed: {0}")]
    PluginRegistry(String),

    #[error("unsafe workspace path `{path}`")]
    UnsafePath { path: String },

    #[error("workspace file `{path}` is too large ({size} bytes, limit {limit})")]
    FileTooLarge {
        path: String,
        size: usize,
        limit: usize,
    },

    #[error("workspace has too many files ({count}, limit {limit})")]
    TooManyFiles { count: usize, limit: usize },

    #[error("cannot read `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot write `{path}`: {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot parse `{path}`: {source}")]
    ParseToml {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("cannot serialize `{path}`: {source}")]
    SerializeToml {
        path: PathBuf,
        source: toml::ser::Error,
    },

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("git operation `{command}` failed: {stderr}")]
    Git { command: String, stderr: String },

    #[error("GitHub operation failed: {0}")]
    GitHub(String),

    #[error("plugin source `{name}` already has a conflicting declaration")]
    ConflictingPluginSource { name: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Evolution(#[from] evolution_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
