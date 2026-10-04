use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct GitRepo {
    path: PathBuf,
}

impl GitRepo {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn init(path: impl AsRef<Path>, default_branch: &str) -> Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let repo = Self {
            path: path.to_path_buf(),
        };
        if !path.join(".git").exists() {
            repo.run(&["init", "--initial-branch", default_branch])?;
        }
        Ok(repo)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_clean(&self) -> Result<bool> {
        let output = self.run_capture(&["status", "--porcelain"])?;
        Ok(output.trim().is_empty())
    }

    pub fn head(&self) -> Result<String> {
        Ok(self.run_capture(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    pub fn remote_url(&self, remote: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["remote", "get-url", remote])
            .output()
            .map_err(|source| self.git_error("remote get-url", source, Vec::new()))?;
        if output.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.run(&["remote", "add", name, url])
    }

    pub fn snapshot_commit(
        &self,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<Option<String>> {
        self.run(&["add", "--all"])?;
        let staged = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["diff", "--cached", "--quiet"])
            .output()
            .map_err(|source| self.git_error("diff --cached --quiet", source, Vec::new()))?;
        if staged.status.success() {
            return Ok(None);
        }
        self.run_configured(&[
            "-c",
            &format!("user.name={author_name}"),
            "-c",
            &format!("user.email={author_email}"),
            "commit",
            "-m",
            message,
        ])?;
        self.head().map(Some)
    }

    pub fn push(&self, remote: &str, branch: &str) -> Result<()> {
        self.run(&["push", "--set-upstream", remote, branch])
    }

    fn run(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .map_err(|source| self.git_error(&args.join(" "), source, Vec::new()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(self.git_error(
                &args.join(" "),
                std::io::Error::other("git exited with failure"),
                output.stderr,
            ))
        }
    }

    fn run_configured(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .map_err(|source| self.git_error(&args.join(" "), source, Vec::new()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(self.git_error(
                &args.join(" "),
                std::io::Error::other("git exited with failure"),
                output.stderr,
            ))
        }
    }

    fn run_capture(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .map_err(|source| self.git_error(&args.join(" "), source, Vec::new()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(self.git_error(
                &args.join(" "),
                std::io::Error::other("git exited with failure"),
                output.stderr,
            ))
        }
    }

    fn git_error(&self, command: &str, source: std::io::Error, stderr: Vec<u8>) -> Error {
        let detail = if stderr.is_empty() {
            source.to_string()
        } else {
            String::from_utf8_lossy(&stderr).trim().to_string()
        };
        Error::Git {
            command: command.to_string(),
            stderr: detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_snapshots_and_reports_clean_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = GitRepo::init(tmp.path(), "main").unwrap();
        assert!(
            repo.snapshot_commit("empty", "Test", "test@example.com")
                .unwrap()
                .is_none()
        );
        std::fs::write(tmp.path().join("README.md"), "# demo\n").unwrap();
        let commit = repo
            .snapshot_commit("snapshot", "Autonomics RSI", "rsi@example.com")
            .unwrap()
            .unwrap();
        assert_eq!(commit.len(), 40);
        assert!(repo.is_clean().unwrap());
        assert!(
            repo.snapshot_commit("noop", "Test", "test@example.com")
                .unwrap()
                .is_none()
        );
    }
}
