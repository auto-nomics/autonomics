//! Durable registry state: schema, versioned decoding, and startup
//! placement resolution.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

use super::{
    backend::{ComputeBackend, ComputeTarget},
    data_source::DataSource,
    persistence::read_private,
};
use crate::RuntimeConfig;

/// Resolve durable placement before opening any legacy backend. The
/// operator's site profile seeds the base config (file > env > default);
/// persisted compute-registry state then overrides placement for enrolled
/// deployments. Environment variables seed the initial state; they do not
/// retarget an enrolled target.
pub(crate) fn startup_config(config: &RuntimeConfig) -> Result<RuntimeConfig, String> {
    let mut config = config.clone();
    let Some(bytes) = read_registry_file(&config.state_dir.join("compute"))? else {
        // Legacy site settings are used only to seed the first registry.
        if let Some(profile) = crate::config::slurm::SlurmSiteProfile::load(&config.state_dir)? {
            profile.apply_to(&mut config.slurm);
        }
        if config.slurm.has_typed_worker() && !config.slurm.enabled {
            return Err("typed-worker placement requires Slurm enabled".into());
        }
        return Ok(config);
    };
    let state = decode_state(&bytes)?;
    let target = state
        .targets
        .iter()
        .find(|t| t.id == state.policy.target_id)
        .ok_or("selected target is missing")?;
    let mut resolved = config.clone();
    match &target.backend {
        ComputeBackend::Local => {
            resolved.slurm.enabled = false;
            resolved.slurm.typed_worker_config = None;
            resolved.slurm.typed_worker = None;
            resolved.slurm.apptainer_manifest = None;
        }
        ComputeBackend::Slurm { settings } => {
            resolved.slurm = settings.clone();
            resolved.slurm.enabled = true;
            resolved.plugin_rsi.environment_dev.enabled = false;
        }
        ComputeBackend::Ssh { settings } => {
            // A direct host has no scheduler keys to honor; only the ssh,
            // scratch, artifact and worker-pinning keys are meaningful.
            resolved.slurm = settings.clone();
            resolved.slurm.enabled = true;
            resolved.plugin_rsi.environment_dev.enabled = false;
        }
    }
    Ok(resolved)
}

/// Read the current registry and reject a state directory containing only an old catalog.
pub(super) fn read_registry_file(root: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = root.join("registry.json");
    match fs::symlink_metadata(&path) {
        Ok(_) => read_private(&path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            match fs::symlink_metadata(root.join("catalog.json")) {
                Ok(_) => Err("unsupported compute catalog: registry.json is required".into()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputePolicy {
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Durable, serializable state owned by the single [`ComputeRegistry`](super::ComputeRegistry).
///
/// This is a storage/transport record, not a second compute-domain object.
pub struct ComputeRegistryState {
    pub version: u32,
    pub policy: ComputePolicy,
    pub targets: Vec<ComputeTarget>,
    /// Standalone sources registered independently of a compute target.
    pub data_sources: Vec<DataSource>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub deleted_target_ids: std::collections::BTreeSet<String>,
}

impl ComputeRegistryState {
    pub(super) fn initial(config: &RuntimeConfig) -> Self {
        let mut targets = vec![ComputeTarget {
            id: "local".into(),
            name: "Local".into(),
            enabled: true,
            backend: ComputeBackend::Local,
            data_source: None,
            retained_data_sources: Vec::new(),
            data_domains: Vec::new(),
            capsule_sha256: BTreeMap::new(),
            endpoints: BTreeMap::new(),
        }];
        if config.slurm.enabled
            || config.slurm.tools_enabled
            || crate::config::slurm::SlurmSiteProfile::path(&config.state_dir).exists()
        {
            targets.push(ComputeTarget {
                id: "legacy-slurm".into(),
                name: "SJTU".into(),
                enabled: config.slurm.enabled,
                backend: ComputeBackend::Slurm {
                    settings: config.slurm.clone(),
                },
                data_source: None,
                retained_data_sources: Vec::new(),
                data_domains: Vec::new(),
                capsule_sha256: BTreeMap::new(),
                endpoints: BTreeMap::new(),
            });
        }
        let target_id = if config.slurm.enabled {
            "legacy-slurm"
        } else {
            "local"
        };
        Self {
            version: 2,
            policy: ComputePolicy {
                target_id: target_id.into(),
            },
            targets,
            data_sources: vec![],
            deleted_target_ids: Default::default(),
        }
    }

    pub fn all_data_sources(&self) -> impl Iterator<Item = &DataSource> {
        self.data_sources
            .iter()
            .filter(|source| {
                !self.targets.iter().any(|target| {
                    self.deleted_target_ids.contains(&target.id)
                        && source.is_retained_mount_for(target)
                })
            })
            .chain(
                self.targets
                    .iter()
                    .filter(|target| !self.deleted_target_ids.contains(&target.id))
                    .flat_map(|target| {
                        target
                            .data_source
                            .iter()
                            .chain(&target.retained_data_sources)
                    }),
            )
    }
}

pub(super) fn decode_state(bytes: &[u8]) -> Result<ComputeRegistryState, String> {
    let state: ComputeRegistryState = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if state.version != 2 {
        return Err("unsupported compute registry state".into());
    }
    for target in &state.targets {
        if state.deleted_target_ids.contains(&target.id) {
            continue;
        }
        match &target.backend {
            ComputeBackend::Slurm { settings } | ComputeBackend::Ssh { settings }
                if settings.typed_worker_config.is_some() =>
            {
                return Err(format!(
                    "compute target '{}' requires an inline typed_worker profile in registry.json",
                    target.id
                ));
            }
            _ => {}
        }
    }
    Ok(state)
}
