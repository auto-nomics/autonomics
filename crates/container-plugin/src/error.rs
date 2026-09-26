use std::io;
use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors are grouped by failure site, and every filesystem variant carries
/// the path involved: a scaffold failure must tell the user what to fix,
/// not replay a raw OS errno.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read current directory: {source}")]
    CurrentDir { source: io::Error },

    #[error("invalid plugin name `{name}`: must be lowercase kebab-case (e.g. `my-tool`)")]
    InvalidName { name: String },

    #[error("refusing to overwrite: `{path}` already exists")]
    AlreadyExists { path: PathBuf },

    #[error("cannot create directory `{path}`: {source}")]
    CreateDir { path: PathBuf, source: io::Error },

    #[error("cannot write `{path}`: {source}")]
    WriteFile { path: PathBuf, source: io::Error },

    // No filesystem context to attach: serialization fails on the model,
    // before any path is opened. `#[from]` keeps `?` working at the call site.
    #[error("cannot serialize manifest: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}
