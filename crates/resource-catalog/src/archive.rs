//! Archive support for resource entries.
//!
//! Resources with physical content (FilePath, Database, Doc) can declare an
//! [`ArchiveSpec`] — the remote location where their content is backed up.
//! The catalog can then [`archive`](ResourceCatalog::archive) (push to remote),
//! [`restore`](ResourceCatalog::restore) (pull from remote), and
//! [`verify`](ResourceCatalog::verify_archive) (check local == remote) via
//! `rclone` subprocess calls.
//!
//! This systematizes the manual "rclone copy + README" convention into a
//! self-describing, programmable workflow: every archivable resource carries
//! its own remote address, and anyone can restore it with a single call.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::catalog::ResourceCatalog;
use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};

// ── Types ────────────────────────────────────────────────────────────────

/// Declarative archive configuration: *where* and *how* to archive a resource.
///
/// Stored on [`ResourceEntry::archive_spec`]. Set once when the resource is
/// registered; the remote address is stable (like the logical name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveSpec {
    /// rclone remote name, e.g. `"aliyun"`, `"tgenome"`.
    pub remote: String,
    /// Remote path (bucket + prefix), e.g. `"autonomics-data/magma/test-data/"`.
    /// Appended to `remote:` to form the full rclone target.
    pub remote_path: String,
    /// Whether to use rclone checksum verification during copy (default true).
    #[serde(default = "default_true")]
    pub checksum: bool,
}

fn default_true() -> bool {
    true
}

impl ArchiveSpec {
    /// Fully-qualified rclone remote path, e.g. `aliyun:autonomics-data/magma/`.
    pub fn remote_target(&self) -> String {
        format!("{}:{}", self.remote, self.remote_path)
    }
}

/// Runtime archive state: tracks the result of the last archive/restore op.
///
/// Updated after each successful [`archive`](ResourceCatalog::archive) or
/// [`restore`](ResourceCatalog::restore) call. Persisted with the manifest so
/// the state survives restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveStatus {
    /// ISO 8601 timestamp of the last successful archive (push to remote).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    /// ISO 8601 timestamp of the last successful restore (pull from remote).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<String>,
    /// Number of files reported by rclone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u64>,
    /// Total size in bytes reported by rclone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Whether the last `verify_archive` confirmed local == remote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
}

impl ArchiveStatus {
    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }
}

impl Default for ArchiveStatus {
    fn default() -> Self {
        Self {
            archived_at: None,
            restored_at: None,
            file_count: None,
            size_bytes: None,
            verified: None,
        }
    }
}

/// Outcome of an archive or restore operation.
#[derive(Debug, Clone)]
pub struct ArchiveOutcome {
    pub files_transferred: u64,
    pub size_bytes: u64,
    pub duration_ms: u128,
}

/// Summary of an archivable resource for `list_archivable`.
#[derive(Debug, Clone)]
pub struct ArchivableResource {
    pub name: String,
    pub kind: crate::kind::ResourceKind,
    pub remote: String,
    pub archived_at: Option<String>,
    pub verified: Option<bool>,
}

// ── ResourceCatalog archive operations ───────────────────────────────────

impl ResourceCatalog {
    // ── Command generation (no side effects) ──────────────────────────

    /// Generate the rclone command that would archive this resource to its
    /// remote, without executing it. Useful for documentation and dry-runs.
    pub fn archive_command(&self, name: &str) -> Result<String> {
        let entry = self.get_entry(name)?;
        let spec = entry.archive_spec.as_ref().ok_or_else(|| {
            ResourceError::Validation(format!("resource '{name}' has no archive_spec"))
        })?;
        let local = self.resolve_local_path(&entry)?;
        let check_flag = if spec.checksum { " --checksum" } else { "" };
        Ok(format!(
            "rclone copy{check_flag} \"{local}\" {remote}",
            local = local.display(),
            remote = spec.remote_target()
        ))
    }

    /// Generate the rclone command that would restore this resource from its
    /// remote, without executing it.
    pub fn restore_command(&self, name: &str) -> Result<String> {
        let entry = self.get_entry(name)?;
        let spec = entry.archive_spec.as_ref().ok_or_else(|| {
            ResourceError::Validation(format!("resource '{name}' has no archive_spec"))
        })?;
        let local = self.resolve_local_path(&entry)?;
        Ok(format!(
            "rclone copy \"{remote}\" \"{local}\"",
            remote = spec.remote_target(),
            local = local.display()
        ))
    }

    // ── Execution ────────────────────────────────────────────────────

    /// Archive (push) a resource to its configured remote via rclone.
    ///
    /// Creates the local directory if it doesn't exist (for Database/Doc),
    /// runs `rclone copy`, queries the remote size, and updates the
    /// `archive_status` on the entry. Non-fatal rclone warnings are logged;
    /// a non-zero exit code is an error.
    pub async fn archive(&self, name: &str) -> Result<ArchiveOutcome> {
        let entry = self.get_entry(name)?;
        let spec = entry.archive_spec.clone().ok_or_else(|| {
            ResourceError::Validation(format!("resource '{name}' has no archive_spec"))
        })?;
        let local = self.resolve_local_path(&entry)?;

        let start = std::time::Instant::now();
        let mut cmd = tokio::process::Command::new("rclone");
        cmd.arg("copy");
        if spec.checksum {
            cmd.arg("--checksum");
        }
        cmd.arg(&local).arg(spec.remote_target());
        let output = cmd
            .output()
            .await
            .map_err(|e| ResourceError::Persistence(format!("failed to spawn rclone: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResourceError::Persistence(format!(
                "rclone archive failed for '{name}': {stderr}"
            )));
        }

        // Query remote size for status tracking.
        let (files, size) = self.rclone_size(&spec.remote_target()).await;
        let outcome = ArchiveOutcome {
            files_transferred: files,
            size_bytes: size,
            duration_ms: start.elapsed().as_millis(),
        };

        // Update archive_status on the entry.
        let prev = self
            .get(name)
            .and_then(|e| e.archive_status.clone())
            .unwrap_or_default();
        let status = ArchiveStatus {
            archived_at: Some(ArchiveStatus::now()),
            restored_at: prev.restored_at,
            file_count: Some(files),
            size_bytes: Some(size),
            verified: None,
        };
        self.update_archive_status(name, status)?;

        tracing::info!(
            resource = name,
            files = files,
            bytes = size,
            "archived to {}",
            spec.remote_target()
        );

        Ok(outcome)
    }

    /// Restore (pull) a resource from its configured remote via rclone.
    ///
    /// Runs `rclone copy remote → local`, creating parent directories as
    /// needed. Updates `restored_at` on the entry.
    pub async fn restore(&self, name: &str) -> Result<ArchiveOutcome> {
        let entry = self.get_entry(name)?;
        let spec = entry.archive_spec.clone().ok_or_else(|| {
            ResourceError::Validation(format!("resource '{name}' has no archive_spec"))
        })?;
        let local = self.resolve_local_path(&entry)?;

        // Ensure parent directory exists.
        if let Some(parent) = local.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let start = std::time::Instant::now();
        let output = tokio::process::Command::new("rclone")
            .arg("copy")
            .arg(spec.remote_target())
            .arg(&local)
            .output()
            .await
            .map_err(|e| ResourceError::Persistence(format!("failed to spawn rclone: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResourceError::Persistence(format!(
                "rclone restore failed for '{name}': {stderr}"
            )));
        }

        let (files, size) = self.local_size(&local);
        let outcome = ArchiveOutcome {
            files_transferred: files,
            size_bytes: size,
            duration_ms: start.elapsed().as_millis(),
        };

        // Update restored_at, preserving other status fields.
        let prev = self
            .get(name)
            .and_then(|e| e.archive_status.clone())
            .unwrap_or_default();
        let status = ArchiveStatus {
            archived_at: prev.archived_at,
            restored_at: Some(ArchiveStatus::now()),
            file_count: Some(files),
            size_bytes: Some(size),
            verified: None,
        };
        self.update_archive_status(name, status)?;

        tracing::info!(
            resource = name,
            files = files,
            bytes = size,
            "restored from {}",
            spec.remote_target()
        );

        Ok(outcome)
    }

    /// Verify that the local content matches the remote (rclone check).
    ///
    /// Updates the `verified` field on the archive status. Returns `true`
    /// if they match, `false` otherwise. Requires both local and remote to
    /// exist.
    pub async fn verify_archive(&self, name: &str) -> Result<bool> {
        let entry = self.get_entry(name)?;
        let spec = entry.archive_spec.clone().ok_or_else(|| {
            ResourceError::Validation(format!("resource '{name}' has no archive_spec"))
        })?;
        let local = self.resolve_local_path(&entry)?;

        let output = tokio::process::Command::new("rclone")
            .arg("check")
            .arg(&local)
            .arg(spec.remote_target())
            .arg("--one-way")
            .output()
            .await
            .map_err(|e| ResourceError::Persistence(format!("failed to spawn rclone: {e}")))?;

        let verified = output.status.success();

        // Update verified field, preserving other status.
        let prev = self
            .get(name)
            .and_then(|e| e.archive_status.clone())
            .unwrap_or_default();
        let status = ArchiveStatus {
            archived_at: prev.archived_at,
            restored_at: prev.restored_at,
            file_count: prev.file_count,
            size_bytes: prev.size_bytes,
            verified: Some(verified),
        };
        self.update_archive_status(name, status)?;

        if verified {
            tracing::info!("resource '{name}' verified: local matches remote");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("resource '{name}' verification FAILED: {stderr}");
        }

        Ok(verified)
    }

    /// List all resources that have an archive spec, with their current status.
    /// Useful for a "what needs archiving?" overview.
    pub fn list_archivable(&self) -> Vec<ArchivableResource> {
        self.list()
            .into_iter()
            .filter_map(|e| {
                let spec = e.archive_spec.clone()?;
                Some(ArchivableResource {
                    name: e.name,
                    kind: e.kind,
                    remote: spec.remote_target(),
                    archived_at: e
                        .archive_status
                        .as_ref()
                        .and_then(|s| s.archived_at.clone()),
                    verified: e.archive_status.as_ref().and_then(|s| s.verified),
                })
            })
            .collect()
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn get_entry(&self, name: &str) -> Result<ResourceEntry> {
        self.get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))
    }

    /// Resolve the local filesystem path for an archivable resource.
    fn resolve_local_path(&self, entry: &ResourceEntry) -> Result<std::path::PathBuf> {
        match &entry.address {
            crate::kind::ResourceAddress::FilePath(p) => Ok(self.absolutize(p)),
            crate::kind::ResourceAddress::Database { path, .. } => Ok(self.absolutize(path)),
            crate::kind::ResourceAddress::Doc { path, .. } => Ok(self.absolutize(path)),
            other => Err(ResourceError::KindMismatch {
                name: entry.name.clone(),
                expected: "file_path/database/doc (archivable)",
                found: crate::resolve::kind_str(other),
            }),
        }
    }

    /// Run `rclone size` on a remote path and parse the output.
    async fn rclone_size(&self, remote: &str) -> (u64, u64) {
        let output = tokio::process::Command::new("rclone")
            .arg("size")
            .arg(remote)
            .arg("--json")
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                parse_rclone_size_json(&text)
            }
            _ => (0, 0),
        }
    }

    /// Count files and total size in a local directory.
    fn local_size(&self, path: &Path) -> (u64, u64) {
        let mut files = 0u64;
        let mut size = 0u64;
        if path.is_file() {
            return (1, path.metadata().map(|m| m.len()).unwrap_or(0));
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        files += 1;
                        size += meta.len();
                    } else if meta.is_dir() {
                        let (f, s) = self.local_size(&entry.path());
                        files += f;
                        size += s;
                    }
                }
            }
        }
        (files, size)
    }

    /// Update the archive_status field on a resource entry (in-place replace).
    fn update_archive_status(&self, name: &str, status: ArchiveStatus) -> Result<()> {
        let mut reg = self.inner.write().expect("catalog lock");
        let entry = reg
            .get_mut(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        entry.archive_status = Some(status);
        Ok(())
    }
}

/// Parse `rclone size --json` output: `{"count":N,"bytes":M}`
fn parse_rclone_size_json(text: &str) -> (u64, u64) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        let count = v.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
        let bytes = v.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
        (count, bytes)
    } else {
        (0, 0)
    }
}
