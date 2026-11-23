//! Data-source lifecycle: declaration, admission and VFS projection.
//!
//! Each source exposes a VFS mount at `/data/<id>` and may define a
//! canonical `<domain>://` address. The registry persists standalone sources
//! and target-owned sources together. Owned sources follow target deletion.

use super::{
    ComputeBackend, ComputeRegistry, ComputeRegistryState, DataDomain, SaveTarget, identifier,
    is_domain_absolute,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataBackend {
    Local {
        root: String,
    },
    Ssh {
        alias: String,
        root: String,
        python: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub backend: DataBackend,
    #[serde(default = "yes")]
    pub read_only: bool,
    /// Filesystem-domain name this source partitions; `<domain>://rest`
    /// then addresses this source's mount. A compute target's DataDomain
    /// validates its declared id against this at enrollment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub endpoint: Option<vfs::SshEndpoint>,
    #[serde(default)]
    pub permissions: vfs::permission::MountPermissions,
}
fn yes() -> bool {
    true
}

/// Compute/source identifiers predate URI domains and admit capitals,
/// underscores and leading digits. Derive the address without renaming the
/// saved source, mount path, or compute target.
pub(crate) fn source_address_domain(id: &str) -> String {
    let scheme = id.to_ascii_lowercase().replace('_', "-");
    if vfs::uri::valid_domain_name(&scheme) && scheme != "local" {
        scheme
    } else {
        format!("source-{scheme}")
    }
}

impl DataSource {
    pub(crate) fn address_domain(&self) -> Option<String> {
        self.domain
            .as_ref()
            .map(|domain| {
                if domain == &self.id {
                    source_address_domain(domain)
                } else {
                    domain.to_ascii_lowercase()
                }
            })
            .or_else(|| {
                matches!(self.backend, DataBackend::Ssh { .. })
                    .then(|| source_address_domain(&self.id))
            })
    }
    fn validate_id(&self) -> Result<(), String> {
        let id = self
            .id
            .strip_prefix("dag-tasks-")
            .filter(|_| self.read_only);
        identifier(&self.id).or_else(|error| match id {
            Some(target_id) => identifier(target_id),
            None => Err(error),
        })
    }

    pub fn mount_path(&self) -> String {
        format!("/data/{}", self.id)
    }
    pub(super) fn matches_target_domain(&self, domains: &[DataDomain]) -> bool {
        let ([domain], DataBackend::Ssh { alias, root, .. }) = (domains, &self.backend) else {
            return false;
        };
        (self.read_only || domain.has_separate_task_mount())
            && self.mount_path() == domain.vfs_prefix
            && self.address_domain().as_deref() == Some(&source_address_domain(&domain.id))
            && root == &domain.remote_root
            && domain.ssh_aliases.contains(alias)
    }

    pub(super) fn is_retained_mount_for(&self, target: &super::ComputeTarget) -> bool {
        target.data_source.is_some()
            && self.read_only
            && target.data_domains.iter().any(|domain| {
                let DataBackend::Ssh { alias, root, .. } = &self.backend else {
                    return false;
                };
                domain.has_separate_task_mount()
                    && domain.retained_vfs_prefix.as_deref() == Some(self.mount_path().as_str())
                    && domain.retained_remote_root.as_ref() == Some(root)
                    && domain.ssh_aliases.contains(alias)
            })
    }

    pub(crate) fn manifest(&self) -> Result<vfs::VfsManifest, String> {
        self.validate_id()?;
        if self.name.trim().is_empty() {
            return Err("data source name is required".into());
        }
        let config = match &self.backend {
            DataBackend::Local { root } => {
                if !Path::new(root).is_absolute() || root == "/" {
                    return Err("data root must be an absolute non-root directory".into());
                }
                if !Path::new(root).is_dir() {
                    return Err("local data root must already be a directory".into());
                }
                vfs::BackendConfig::local(root)
            }
            DataBackend::Ssh {
                alias,
                root,
                python,
            } => {
                // The VFS applies the mount permission policy independently
                // of the SSH filesystem permissions.
                vfs::BackendConfig::Ssh {
                    alias: alias.clone(),
                    root: root.clone(),
                    python: python.clone(),
                    endpoint: self.endpoint.clone(),
                }
            }
        };
        Ok(vfs::VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: format!("data-{}", self.id),
                config,
            }],
            mount: vec![vfs::MountDefinition {
                domain: self.address_domain(),
                path: self.mount_path(),
                backend: format!("data-{}", self.id),
                source: "/".into(),
                read_only: self.read_only,
                permissions: self.permissions.clone(),
            }],
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDataSource {
    pub source: DataSource,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveTargetDataSource {
    pub target: SaveTarget,
    pub alias: String,
    pub root: String,
    pub python: String,
}

impl ComputeRegistry {
    pub async fn save_target_data_source(
        &self,
        mut request: SaveTargetDataSource,
    ) -> Result<ComputeRegistryState, String> {
        self.healthy()?;
        let state = self.state();
        let target = state
            .targets
            .iter()
            .find(|target| {
                target.id == request.target.id && !state.deleted_target_ids.contains(&target.id)
            })
            .ok_or("unknown target")?;
        let settings = match (&target.backend, &mut request.target.backend) {
            (ComputeBackend::Ssh { .. }, ComputeBackend::Ssh { settings })
            | (ComputeBackend::Slurm { .. }, ComputeBackend::Slurm { settings }) => settings,
            _ => return Err("editing a data mount requires the existing remote backend".into()),
        };
        if request.target.name.trim().is_empty() || request.python.trim().is_empty() {
            return Err("target name and remote Python are required".into());
        }
        if target.data_source.as_ref().is_some_and(|source| {
            matches!(&source.backend, DataBackend::Ssh { alias, root, python }
                if alias == &request.alias && root == &request.root && python == &request.python)
        }) {
            return self.save_target_with_source(request.target, None, Some(target));
        }

        if target
            .data_domains
            .iter()
            .any(DataDomain::has_separate_task_mount)
        {
            return Err("changing the unified storage root requires an offline migration".into());
        }

        // Keep the old URI and mount path stable for already-bound work.
        let id = format!("mount-{}", uuid::Uuid::new_v4().simple());
        let domain = DataDomain {
            id: id.clone(),
            ssh_aliases: vec![request.alias.clone()],
            remote_root: request.root.clone(),
            vfs_prefix: format!("/data/{id}"),
            retained_vfs_prefix: None,
            retained_remote_root: None,
        };
        domain.validate()?;

        let scratch = Path::new(&settings.scratch_remote_dir);
        let new_root = Path::new(&request.root);
        if !is_domain_absolute(&settings.scratch_remote_dir)
            || scratch == new_root
            || !scratch.starts_with(new_root)
        {
            let relative = target
                .data_domains
                .first()
                .and_then(|domain| scratch.strip_prefix(&domain.remote_root).ok())
                .filter(|suffix| {
                    !suffix.as_os_str().is_empty()
                        && is_domain_absolute(&settings.scratch_remote_dir)
                });
            settings.scratch_remote_dir = new_root
                .join(relative.unwrap_or_else(|| Path::new("scratch")))
                .to_string_lossy()
                .into_owned();
        }
        settings.data_ssh_host = Some(request.alias.clone());
        request.target.data_domains = vec![domain];
        let source = DataSource {
            id: id.clone(),
            name: target
                .data_source
                .as_ref()
                .map(|source| source.name.clone())
                .unwrap_or_else(|| format!("{} shared filesystem", target.id)),
            backend: DataBackend::Ssh {
                alias: request.alias,
                root: request.root,
                python: request.python,
            },
            read_only: true,
            domain: Some(id.clone()),
            endpoint: None,
            permissions: target
                .data_source
                .as_ref()
                .map(|source| source.permissions.clone())
                .unwrap_or_default(),
        };
        let source = self.prepare_source(source).await?;
        self.save_target_with_source(request.target, Some((source, None)), Some(target))
    }

    pub(super) async fn prepare_source(
        &self,
        mut source: DataSource,
    ) -> Result<DataSource, String> {
        self.healthy()?;
        source.validate_id()?;
        source.domain = source.address_domain();
        self.storage
            .require_vacant_mount_path(&source.mount_path())
            .await
            .map_err(|e| e.to_string())?;
        if let DataBackend::Ssh { alias, .. } = &source.backend {
            source.endpoint = Some(vfs::SshEndpoint::capture("ssh", alias)?);
        }
        Ok(source)
    }

    pub(super) fn mount_source(
        &self,
        state: &ComputeRegistryState,
        source: &DataSource,
    ) -> Result<(), String> {
        if state
            .data_sources
            .iter()
            .chain(state.targets.iter().flat_map(|target| {
                target
                    .data_source
                    .iter()
                    .chain(&target.retained_data_sources)
            }))
            .any(|existing| existing.id == source.id)
        {
            return Err("data source identity already exists; use a new namespace".into());
        }
        self.vfs
            .add_data_mount(&source.manifest()?)
            .map_err(|error| error.to_string())
    }

    pub async fn add_source(
        &self,
        request: SaveDataSource,
    ) -> Result<ComputeRegistryState, String> {
        let source = self.prepare_source(request.source).await?;
        let _guard = self.mutation.lock().map_err(|_| "compute lock poisoned")?;
        self.healthy()?;
        let mut next = self.state();
        self.mount_source(&next, &source)?;
        let path = source.mount_path();
        next.data_sources.push(source);
        self.persist(&next).inspect_err(|_| {
            let _ = self.vfs.remove_data_mounts(&[path]);
        })?;
        *self.state.write().unwrap() = next.clone();
        Ok(next)
    }
}
