//! Compute backend declarations and the persisted target record.
use crate::config::SlurmConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{data_source::DataSource, domain::DataDomain};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ComputeBackend {
    Local,
    Slurm {
        settings: SlurmConfig,
    },
    /// A scheduler-free remote host: tasks run directly over ssh with no
    /// queue, no priority and no enforced resource ceiling. The settings
    /// reuses the `SlurmConfig` carrier for its ssh/scratch/artifact keys;
    /// the scheduler-only keys (partition, account, qos, gpu_*, mem_per_cpu)
    /// are ignored here and the editors do not offer them.
    Ssh {
        settings: SlurmConfig,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeTarget {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub backend: ComputeBackend,
    /// Data source provisioned for this target. Deleted target records reserve
    /// the mount identity for history and no longer expose it through VFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<DataSource>,
    /// Task artifacts and previous mounts whose URIs remain valid until target deletion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_data_sources: Vec<DataSource>,
    /// Filesystem domains this target references in place. Phase 1 of
    /// the placement model: the domain declaration moves from the
    /// typed-worker profile's free-form `data_home` field into the
    /// target record, where registry state pins endpoint
    /// identity alongside the other target aliases. Empty for local
    /// targets and pre-existing enrollments (the profile field remains
    /// the legacy fallback).
    #[serde(default)]
    pub data_domains: Vec<DataDomain>,
    #[serde(default)]
    pub capsule_sha256: BTreeMap<String, String>,
    #[serde(default)]
    pub endpoints: BTreeMap<String, vfs::SshEndpoint>,
}

pub(crate) fn same_job_connection(left: &ComputeTarget, right: &ComputeTarget) -> bool {
    let connection = |backend: &ComputeBackend| match backend {
        ComputeBackend::Slurm { settings } => Some((
            settings.ssh_host.clone(),
            settings.data_ssh_host.clone(),
            settings.pi_ssh_host.clone(),
        )),
        _ => None,
    };
    connection(&left.backend) == connection(&right.backend) && left.endpoints == right.endpoints
}
