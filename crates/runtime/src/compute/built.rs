//! The built (resident) form of a target plus the run-binding and
//! evidence records derived from it.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

use super::{
    backend::{ComputeBackend, ComputeTarget},
    domain::DataDomain,
    state::ComputeRegistryState,
};
use container_runtime::{ContainerImageResolution, PodmanConnection};
use dag_core::TaskExecutor;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeStatus {
    #[serde(default)]
    pub deployment: serde_json::Value,
    pub state: ComputeRegistryState,
    pub mounts: Vec<vfs::MountDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub id: String,
    pub target_id: String,
    pub target_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeImageEvidence {
    pub target_id: String,
    pub target_sha256: String,
    pub capsule_sha256: BTreeMap<String, String>,
    pub resolution: ContainerImageResolution,
    pub evidence_scope: String,
    pub sif_bytes_verified: bool,
    pub node_execution_performed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveTarget {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub backend: ComputeBackend,
    /// Declared placement domains. The transport currently admits at
    /// most one; the vec keeps the registry-state schema forward-compatible.
    #[serde(default)]
    pub data_domains: Vec<DataDomain>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectTarget {
    pub target_id: String,
}
pub(crate) struct BuiltTarget {
    pub(crate) typed_worker_revision: Option<String>,
    pub(crate) network_policy_sha256: Option<String>,
    pub target: ComputeTarget,
    pub executor: Arc<dyn TaskExecutor>,
    pub containers: Arc<dyn PodmanConnection>,
    pub hpc: Option<Arc<hpc_tools::HpcClient>>,
    /// The direct-host ssh identity, so a configuration check can observe
    /// the connection a run would actually use. Absent on other backends.
    pub ssh_probe: Option<dag_core::SshTarget>,
}

impl BuiltTarget {
    pub(crate) fn deployment(&self) -> serde_json::Value {
        let source_compatible = self
            .typed_worker_revision
            .as_deref()
            .is_none_or(|revision| revision == dag_core::source_revision());
        serde_json::json!({
            "target_id": self.target.id,
            "target_sha256": serde_json::to_vec(&self.target).ok().map(|bytes| format!("{:x}", Sha256::digest(bytes))),
            "coordinator_source_revision": dag_core::source_revision(),
            "worker_source_revision": self.typed_worker_revision,
            "source_compatible": source_compatible,
            "scope": "COORDINATOR_WORKER_SOURCE_COMPATIBILITY_ONLY",
            "workflow_readiness": "NOT_CHECKED_USE_COMPUTE_READINESS",
            "worker_and_image_execution_verified": false,
            "error": if source_compatible { None } else { Some("typed_worker_source_mismatch") },
        })
    }

    pub(crate) fn ensure_source_compatible(&self) -> Result<(), String> {
        if let Some(revision) = &self.typed_worker_revision
            && revision != dag_core::source_revision()
        {
            return Err(format!(
                "compute target {} has typed-worker source {}; coordinator source is {}. Deploy the measured matching worker and update the target before starting a run",
                self.target.id,
                revision,
                dag_core::source_revision(),
            ));
        }
        Ok(())
    }
}
