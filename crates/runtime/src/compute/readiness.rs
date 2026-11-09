//! Read-only deployment requirements. Success here proves configuration only.
use super::*;
use serde_json::{Value, json};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessRequest {
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub images: Vec<String>,
    /// Optional exact expected factory identities, keyed by plugin/node address.
    #[serde(default)]
    pub expected_identities: BTreeMap<String, Value>,
}

impl ComputeRegistry {
    pub fn readiness(
        &self,
        request: &ReadinessRequest,
        nodes: &[dag_core::registry::NodeInfo],
    ) -> Result<Value, String> {
        if request.nodes.len() + request.images.len() > 256
            || request.expected_identities.len() > 256
        {
            return Err("at most 256 readiness requirements allowed".into());
        }
        for (address, identity) in &request.expected_identities {
            if !request.nodes.contains(address)
                || !identity.is_object()
                || !identity["manifest_sha256"].is_string()
                || !identity["image_reference"].is_string()
                || identity.get("script_sha256").is_none()
            {
                return Err(
                    "expected identities require a requested node and manifest/script/image fields"
                        .into(),
                );
            }
        }
        self.healthy()?;
        let state = self.state();
        let policy = &state.policy;
        let target = self.target(&policy.target_id, Some(policy.target_revision))?;
        let mut blockers = Vec::new();
        let mut checks = Vec::new();
        if let Err(error) = target.ensure_source_compatible() {
            blockers.push(json!({"code":"WORKER_SOURCE_MISMATCH","detail":error}));
        }
        if !state
            .targets
            .iter()
            .rev()
            .find(|t| t.id == policy.target_id)
            .is_some_and(|t| t.enabled)
        {
            blockers.push(json!({"code":"TARGET_DISABLED"}));
        }
        let mut images: std::collections::BTreeSet<String> =
            request.images.iter().cloned().collect();
        let remote = matches!(
            target.target.backend,
            ComputeBackend::Slurm { .. } | ComputeBackend::Ssh { .. }
        );
        for address in request
            .nodes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let found: Vec<_> = nodes.iter().filter(|n| &n.address == address).collect();
            if found.len() != 1 {
                blockers.push(json!({"code":"NODE_NOT_REGISTERED","node":address}));
                continue;
            }
            let node = found[0];
            if remote
                && node.resident_plugin.is_none()
                && matches!(
                    node.kind.as_str(),
                    "http_fetch" | "file_download" | "http_head"
                )
            {
                match &target.network_policy_sha256 {
                    Some(pin) => checks.push(json!({"node":address,"network_policy_sha256":pin,
                        "state":"NETWORK_POLICY_PINNED","remote_policy_verified":false})),
                    None => blockers.push(json!({"code":"WORKER_NETWORK_POLICY_UNCONFIGURED","node":address,
                        "next_action":"Prepare an owned private environment with the approved policy and enroll its SHA256 in a new immutable worker target"})),
                }
            }
            if let Some(resident) = &node.resident_plugin {
                images.insert(resident.identity.image_reference.clone());
                let actual = serde_json::to_value(&resident.identity).map_err(|e| e.to_string())?;
                if let Some(expected) = request.expected_identities.get(address) {
                    if expected
                        .as_object()
                        .unwrap()
                        .iter()
                        .any(|(key, value)| actual.get(key).unwrap_or(&Value::Null) != value)
                    {
                        blockers.push(json!({"code":"RESIDENT_IDENTITY_MISMATCH","node":address,
                            "expected":expected,"actual":actual}));
                    }
                }
                checks.push(json!({"node":address,"resident_plugin":resident}));
            } else if request.expected_identities.contains_key(address) {
                blockers.push(json!({"code":"RESIDENT_IDENTITY_UNAVAILABLE","node":address}));
            } else {
                checks.push(json!({"node":address,"state":"REGISTERED_BUILTIN",
                    "dynamic_requirements":"CHECKED_FROM_COMPILED_NODE_AT_DISPATCH"}));
            }
        }
        for image in images {
            if !remote {
                checks
                    .push(json!({"image":image,"state":"NOT_CHECKED","scope":"LOCAL_IMAGE_STORE"}));
                continue;
            }
            match target.containers.image_resolution(&image) {
                Ok(resolution) => checks.push(json!({"image":image,"state":"MAPPED","resolution":resolution})),
                Err(error) => blockers.push(json!({"code":"IMAGE_MAPPING_UNAVAILABLE","image":image,
                    "detail":error.to_string(),"next_action":"Enroll and select a new immutable target with the qualified OCI-to-SIF binding"})),
            }
        }
        // Reject a mixed report when policy changes while requirements are inspected.
        if self.state().revision != state.revision {
            return Err(
                "compute configuration changed during readiness inspection; read again".into(),
            );
        }
        Ok(
            json!({"state":if !blockers.is_empty() { "BLOCKED" } else if remote && !checks.is_empty() { "CONFIGURATION_READY" } else { "NOT_VERIFIED" },
            "scope":"REQUESTED_RESIDENT_NODES_AND_IMAGE_MAPPINGS",
            "verification_gaps": ["Runtime history persistence and artifact access are outside this configuration check", "No execution is performed; empty image/identity requirements verify no images/identities"],
            "requested_images":request.images.len(), "requested_identities":request.expected_identities.len(), "gateway_pid":std::process::id(),
            "registry_revision":state.revision,"policy":policy,"deployment":target.deployment(),
            "target_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&target.target).map_err(|e|e.to_string())?)),
            "capsule_sha256":target.target.capsule_sha256,"checks":checks,"blockers":blockers,
            "remote_sif_bytes_verified":false,"end_to_end_acceptance":"NOT_VERIFIED",
            "data_access_verified":false,"scientific_authorization_granted":false,"jobs_submitted":false}),
        )
    }
}
