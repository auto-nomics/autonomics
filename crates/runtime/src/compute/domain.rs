//! Filesystem-domain vocabulary shared by compute targets and data
//! sources.
//!
//! A domain is the neutral naming layer of the placement model: a
//! [`DataDomain`] is a *target-side* declaration of the cluster
//! filesystem its compute nodes see, while a data source projects one
//! backend as the coordinator-side read-only (or operator-opted
//! writable) mount such a declaration references. Keeping the record
//! here — rather than in the target or the source module — means both
//! sides depend on this module and never on each other.

use super::data_source::source_address_domain;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A filesystem domain a remote compute target references in place.
///
/// Aliases are operator assertions that these SSH endpoints share the
/// filesystem the target's compute nodes see; the executor re-verifies
/// every in-place reference against the live endpoint before trusting
/// it. `vfs_prefix` is the coordinator-side projection mount under
/// which published references surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataDomain {
    pub id: String,
    pub ssh_aliases: Vec<String>,
    pub remote_root: String,
    pub vfs_prefix: String,
    /// Optional dedicated output namespace. Both fields must be supplied;
    /// inputs remain confined to `remote_root` and its admitted aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_vfs_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_remote_root: Option<String>,
}

impl DataDomain {
    /// Admission rules mirroring the transport-side `SshDataHome`
    /// validation so an invalid domain fails the registry save, not the
    /// first scheduled task.
    pub(super) fn has_separate_task_mount(&self) -> bool {
        let data = Path::new(&self.remote_root);
        self.retained_remote_root.as_deref().is_some_and(|root| {
            let tasks = Path::new(root);
            data.ends_with("data")
                && tasks.ends_with("dag-tasks")
                && data.parent() == tasks.parent()
                && self.retained_vfs_prefix.as_deref().is_some_and(|prefix| {
                    prefix.starts_with("/data/tasks-")
                        || self
                            .vfs_prefix
                            .strip_prefix("/data/")
                            .is_some_and(|id| prefix.strip_prefix("/data/dag-tasks-") == Some(id))
                })
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.len() > 64 || !vfs::uri::valid_domain_name(&self.id) || self.id == "local" {
            return Err("remote data domain id must be a lowercase, non-reserved URI scheme other than local".into());
        }
        if self.ssh_aliases.is_empty()
            || self.ssh_aliases.iter().any(|alias| {
                alias.is_empty()
                    || alias.starts_with('-')
                    || !alias
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c))
            })
        {
            return Err("data domain requires at least one valid ssh alias".into());
        }
        domain_absolute(&self.remote_root, "remote_root")?;
        domain_absolute(&self.vfs_prefix, "vfs_prefix")?;
        match (&self.retained_vfs_prefix, &self.retained_remote_root) {
            (None, None) => {}
            (Some(prefix), Some(root))
                if is_domain_absolute(prefix)
                    && is_domain_absolute(root)
                    && prefix != &self.vfs_prefix
                    && root != &self.remote_root
                    && (Path::new(root).starts_with(&self.remote_root)
                    || self.has_separate_task_mount()) => {}
            _ => {
                return Err(
                    "retained output prefix/root must be a paired, distinct namespace below the data domain root"
                        .into(),
                )
            }
        }
        Ok(())
    }

    /// The transport-side placement record for this domain. Historical ids
    /// use the same URI spelling as their source mounts, without rewriting
    /// target records or their pinned profiles.
    pub fn data_home(&self) -> dag_core::SshDataHome {
        dag_core::SshDataHome {
            ssh_aliases: self.ssh_aliases.clone(),
            remote_root: self.remote_root.clone(),
            vfs_prefix: self.vfs_prefix.clone(),
            domain_id: Some(source_address_domain(&self.id)),
            retained_vfs_prefix: self.retained_vfs_prefix.clone(),
            retained_remote_root: self.retained_remote_root.clone(),
        }
    }
}

pub(crate) fn is_domain_absolute(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.ends_with('/')
        && !path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && !path.chars().any(char::is_control)
}

fn domain_absolute(path: &str, field: &str) -> Result<(), String> {
    if !is_domain_absolute(path) {
        return Err(format!("data domain {field} must be a clean absolute path"));
    }
    Ok(())
}
