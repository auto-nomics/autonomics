//! Error type for the skills crate.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// The candidate directory has no `SKILL.md`.
    #[error("no SKILL.md found under {0}")]
    MissingSkillMd(PathBuf),

    /// SKILL.md could not be read or decoded.
    #[error("cannot read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },

    /// Frontmatter exists but violates the format contract.
    #[error("invalid SKILL.md at {path}: {reason}")]
    InvalidFrontmatter { path: PathBuf, reason: String },

    /// The frontmatter `name` is not a valid skill name.
    #[error("invalid skill name {0:?}: must be lowercase kebab-case, 1-64 chars")]
    InvalidName(String),

    #[error("skill {0:?} not found")]
    NotFound(String),

    /// Built-in skills ship with the binary and cannot be uninstalled.
    #[error("{0} is a built-in skill and cannot be uninstalled")]
    Builtin(String),

    /// An install source could not be interpreted.
    #[error(
        "cannot parse skill source {0:?} \
             (expected a local path, https://github.com/owner/repo, \
             github.com/owner/repo/tree/ref/path, or owner/repo@path)"
    )]
    BadSource(String),

    /// A `git` invocation failed or is unavailable.
    #[error("git: {0}")]
    Git(String),

    /// The install manifest sidecar is malformed. Installation continues
    /// without provenance rather than aborting on a corrupt sidecar.
    #[error("malformed install manifest at {0}")]
    BadManifest(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Evolution(#[from] evolution_core::Error),
}

impl SkillError {
    /// Attach a path to a bare reason string from a frontmatter violation.
    pub fn invalid_frontmatter(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::InvalidFrontmatter {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
