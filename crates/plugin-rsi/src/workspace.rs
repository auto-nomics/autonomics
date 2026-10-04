use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{Error, Result, request::unix_now};

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_FILES: usize = 256;

/// A daemon-owned plugin repository. Agent-facing APIs accept only relative
/// paths under this root and can never address `.git` or host paths.
#[derive(Debug, Clone)]
pub struct ProposalWorkspace {
    repo_path: PathBuf,
}

impl ProposalWorkspace {
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.repo_path
    }

    pub fn write_text(&self, relative: &str, contents: &str) -> Result<()> {
        let path = self.safe_path(relative)?;
        let size = contents.len();
        if size > MAX_FILE_BYTES {
            return Err(Error::FileTooLarge {
                path: relative.to_string(),
                size,
                limit: MAX_FILE_BYTES,
            });
        }
        self.assert_file_budget()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            ensure_no_symlink_parent(&self.repo_path, parent)?;
        }
        if path.symlink_metadata().is_ok() {
            ensure_not_symlink(&path)?;
        }

        let tmp = path.with_file_name(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file"),
            temp_suffix()
        ));
        std::fs::write(&tmp, contents).map_err(|source| Error::WriteFile {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::WriteFile {
            path: path.clone(),
            source,
        })
    }

    pub fn read_text(&self, relative: &str) -> Result<String> {
        let path = self.safe_path(relative)?;
        ensure_not_symlink(&path)?;
        std::fs::read_to_string(&path).map_err(|source| Error::ReadFile { path, source })
    }

    pub fn list_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        visit_files(&self.repo_path, &self.repo_path, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn safe_path(&self, relative: &str) -> Result<PathBuf> {
        if relative.is_empty() || relative.contains('\0') {
            return Err(Error::UnsafePath {
                path: relative.to_string(),
            });
        }
        let candidate = Path::new(relative);
        if candidate.is_absolute()
            || !candidate
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(Error::UnsafePath {
                path: relative.to_string(),
            });
        }
        if candidate.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name == ".git")
        }) {
            return Err(Error::UnsafePath {
                path: relative.to_string(),
            });
        }
        Ok(self.repo_path.join(candidate))
    }

    fn assert_file_budget(&self) -> Result<()> {
        if !self.repo_path.is_dir() {
            return Ok(());
        }
        let count = self.list_files()?.len();
        if count >= MAX_FILES {
            return Err(Error::TooManyFiles {
                count,
                limit: MAX_FILES,
            });
        }
        Ok(())
    }
}

fn visit_files(root: &Path, dir: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        ensure_not_symlink(&path)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        if path.is_dir() {
            visit_files(root, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .map_err(|_| Error::UnsafePath {
                    path: path.display().to_string(),
                })?;
            files.push(relative);
        }
    }
    Ok(())
}

fn ensure_not_symlink(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(Error::UnsafePath {
            path: path.display().to_string(),
        });
    }
    Ok(())
}

fn ensure_no_symlink_parent(root: &Path, mut path: &Path) -> Result<()> {
    while path.starts_with(root) && path != root {
        ensure_not_symlink(path)?;
        path = path.parent().ok_or_else(|| Error::UnsafePath {
            path: path.display().to_string(),
        })?;
    }
    Ok(())
}

fn temp_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("{}-{}", unix_now(), NEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape_and_git_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = ProposalWorkspace::new(tmp.path());
        for path in [
            "/etc/passwd",
            "../escape",
            "./dot",
            ".git/config",
            "scripts/.git/hooks/pre-push",
        ] {
            assert!(workspace.write_text(path, "x").is_err(), "{path}");
        }
    }

    #[test]
    fn writes_and_lists_without_git_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = ProposalWorkspace::new(tmp.path());
        workspace.write_text("manifest.toml", "x = 1").unwrap();
        workspace
            .write_text("scripts/adapter.sh", "set -eu\n")
            .unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert_eq!(
            workspace.list_files().unwrap(),
            vec!["manifest.toml", "scripts/adapter.sh"]
        );
        assert_eq!(workspace.read_text("manifest.toml").unwrap(), "x = 1");
    }

    #[test]
    fn refuses_existing_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = ProposalWorkspace::new(tmp.path());
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("payload"),
            tmp.path().join("scripts/adapter.sh"),
        )
        .unwrap();
        assert!(workspace.write_text("scripts/adapter.sh", "safe").is_err());
    }
}
