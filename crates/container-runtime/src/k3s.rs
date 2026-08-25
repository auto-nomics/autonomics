//! Single-container Kubernetes Jobs driven by a k3s cluster.
//!
//! Workspaces and immutable panel caches are shared through PVCs. The runtime
//! never asks Kubernetes for a host bind mount; node placement can therefore
//! become data-aware without changing the container_command contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobCondition, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EmptyDirVolumeSource, EnvVar, PersistentVolumeClaimVolumeSource, Pod,
    PodSecurityContext, PodTemplateSpec, ResourceRequirements, SeccompProfile, SecurityContext,
    Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use kube::api::{Api, DeleteParams, ListParams, LogParams, PostParams};
use kube::{Client, Config, config::KubeConfigOptions};
use tokio::sync::OnceCell;

use crate::error::ContainerRuntimeError;
use crate::runtime::{ContainerRuntime, MAX_CAPTURED_OUTPUT_BYTES};
use crate::types::{ContainerRunRequest, ContainerRunResult, WorkspaceRef};

const JOB_LABEL: &str = "autonomics.io/job-name";
const RUN_LABEL: &str = "autonomics.io/run";
const NETWORK_LABEL: &str = "autonomics.io/network";
const DEFAULT_K3S_STATE_ROOT: &str = "/var/lib/autonomics/k3s";

#[derive(Debug, Clone)]
pub struct K3sConfig {
    pub namespace: String,
    pub context: Option<String>,
    pub workspace_pvc: String,
    pub workspace_root: PathBuf,
    pub panel_pvc: String,
    pub panel_cache_root: PathBuf,
    pub panel_pvc_prefix: String,
    pub service_account: Option<String>,
    pub poll_interval_ms: u64,
}

impl Default for K3sConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl K3sConfig {
    pub fn from_env() -> Self {
        // Keep these defaults aligned with the local PV paths in
        // `infra/k3s/manifests.yaml`; the control process and kubelet must
        // resolve both volume roots to the same host directories.
        Self {
            namespace: env_value("AUTONOMICS_K3S_NAMESPACE", "autonomics"),
            context: std::env::var_os("AUTONOMICS_K3S_CONTEXT")
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string_lossy().into_owned()),
            workspace_pvc: env_value("AUTONOMICS_K3S_WORKSPACE_PVC", "autonomics-workspace"),
            workspace_root: env_path(
                "AUTONOMICS_K3S_WORKSPACE_ROOT",
                Path::new(DEFAULT_K3S_STATE_ROOT).join("workspace"),
            ),
            panel_pvc: env_value("AUTONOMICS_K3S_PANEL_PVC", "autonomics-panels"),
            panel_cache_root: env_path(
                "AUTONOMICS_PANEL_CACHE_ROOT",
                Path::new(DEFAULT_K3S_STATE_ROOT).join("panels"),
            ),
            panel_pvc_prefix: std::env::var("AUTONOMICS_K3S_PANEL_PVC_PREFIX").unwrap_or_default(),
            service_account: std::env::var_os("AUTONOMICS_K3S_SERVICE_ACCOUNT")
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string_lossy().into_owned()),
            poll_interval_ms: env_value("AUTONOMICS_K3S_POLL_INTERVAL_MS", "500")
                .parse()
                .unwrap_or(500),
        }
    }
}

impl Default for K3sRuntime {
    fn default() -> Self {
        Self::from_env()
    }
}

fn env_value(key: &str, default: &str) -> String {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| default.to_string())
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

pub struct K3sRuntime {
    config: K3sConfig,
    client: OnceCell<Client>,
}

impl K3sRuntime {
    pub fn new(config: K3sConfig) -> Self {
        Self {
            config,
            client: OnceCell::new(),
        }
    }

    pub fn from_env() -> Self {
        Self::new(K3sConfig::from_env())
    }

    pub fn config(&self) -> &K3sConfig {
        &self.config
    }

    pub fn workspace_root(&self) -> &Path {
        &self.config.workspace_root
    }

    async fn client(&self) -> Result<&Client, ContainerRuntimeError> {
        self.client
            .get_or_try_init(|| async {
                let mut options = KubeConfigOptions::default();
                if let Some(context) = &self.config.context {
                    options.context = Some(context.clone());
                }
                let config = if self.config.context.is_some() {
                    Config::from_kubeconfig(&options).await.map_err(|error| {
                        ContainerRuntimeError::Invalid(format!(
                            "cannot load k3s kubeconfig context: {error}"
                        ))
                    })?
                } else {
                    Config::infer().await.map_err(|error| {
                        ContainerRuntimeError::Invalid(format!(
                            "cannot infer k3s client configuration: {error}"
                        ))
                    })?
                };
                Ok(Client::try_from(config)?)
            })
            .await
    }

    async fn job_api(&self) -> Result<Api<Job>, ContainerRuntimeError> {
        Ok(Api::namespaced(
            self.client().await?.clone(),
            &self.config.namespace,
        ))
    }

    async fn pod_api(&self) -> Result<Api<Pod>, ContainerRuntimeError> {
        Ok(Api::namespaced(
            self.client().await?.clone(),
            &self.config.namespace,
        ))
    }

    async fn collect_logs(&self, job_name: &str) -> String {
        let Ok(pods) = self.pod_api().await else {
            return String::new();
        };
        let list = pods
            .list(&ListParams::default().labels(&format!("{RUN_LABEL}={job_name}")))
            .await;
        let Ok(list) = list else {
            return String::new();
        };
        for pod in list.items {
            if let Some(name) = pod.metadata.name {
                if let Ok(logs) = pods.logs(&name, &LogParams::default()).await {
                    if !logs.trim().is_empty() {
                        return truncate_log(&logs);
                    }
                }
            }
        }
        String::new()
    }

    async fn delete_job(&self, job_name: &str) {
        if let Ok(jobs) = self.job_api().await {
            let _ = jobs.delete(job_name, &DeleteParams::default()).await;
        }
    }
}

#[async_trait]
impl ContainerRuntime for K3sRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        validate_request(&request)?;
        let job = build_job(&request, &self.config);
        let jobs = self.job_api().await?;
        jobs.create(&PostParams::default(), &job).await?;

        let finished = tokio::time::timeout(
            Duration::from_secs(request.timeout_secs),
            wait_for_job(&jobs, &request.name, self.config.poll_interval_ms),
        )
        .await;
        let logs = self.collect_logs(&request.name).await;
        let result = match finished {
            Ok(Ok(None)) => Ok(ContainerRunResult {
                exit_code: 0,
                stdout: logs,
                stderr: String::new(),
            }),
            Ok(Ok(conditions)) => Err(ContainerRuntimeError::ExitStatus {
                exit_code: 1,
                stderr: failed_condition_message(conditions.as_deref(), &logs),
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ContainerRuntimeError::Timeout {
                timeout_secs: request.timeout_secs,
            }),
        };
        self.delete_job(&request.name).await;
        result
    }

    fn name(&self) -> &'static str {
        "k3s"
    }

    fn workspace_root(&self) -> &Path {
        self.config.workspace_root.as_path()
    }
}

async fn wait_for_job(
    jobs: &Api<Job>,
    name: &str,
    poll_interval_ms: u64,
) -> Result<Option<Vec<JobCondition>>, ContainerRuntimeError> {
    loop {
        let job = jobs.get(name).await?;
        if let Some(status) = job.status {
            if status.succeeded.unwrap_or_default() > 0 {
                return Ok(None);
            }
            if status.failed.unwrap_or_default() > 0 {
                return Ok(status.conditions);
            }
        }
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;
    }
}

pub(crate) fn build_job(request: &ContainerRunRequest, config: &K3sConfig) -> Job {
    let mut volumes = vec![
        Volume {
            name: "workspace".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: config.workspace_pvc.clone(),
                read_only: Some(false),
            }),
            ..Default::default()
        },
        Volume {
            name: "tmp".into(),
            empty_dir: Some(EmptyDirVolumeSource::default()),
            ..Default::default()
        },
    ];
    if let Some(shm_size) = &request.shm_size {
        volumes.push(Volume {
            name: "shm".into(),
            empty_dir: Some(EmptyDirVolumeSource {
                size_limit: Some(k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                    shm_size.clone(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        });
    }

    let mut mounts = vec![
        VolumeMount {
            name: "workspace".into(),
            mount_path: request.workspace.container_workdir.clone(),
            sub_path: Some(request.workspace.pvc_sub_path.clone()),
            read_only: Some(false),
            ..Default::default()
        },
        VolumeMount {
            name: "tmp".into(),
            mount_path: "/tmp".into(),
            read_only: Some(false),
            ..Default::default()
        },
    ];
    if request.shm_size.is_some() {
        mounts.push(VolumeMount {
            name: "shm".into(),
            mount_path: "/dev/shm".into(),
            read_only: Some(false),
            ..Default::default()
        });
    }
    for (index, panel) in request.panels.iter().enumerate() {
        let volume_name = format!("panel-{index}");
        volumes.push(Volume {
            name: volume_name.clone(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: config.panel_pvc.clone(),
                read_only: Some(true),
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: volume_name,
            mount_path: panel.mount_path.clone(),
            sub_path: Some(panel.pvc_sub_path.clone()),
            read_only: Some(true),
            ..Default::default()
        });
    }

    let (uid, gid) = user_ids(request);
    let resources = resource_requirements(request);
    let container = Container {
        name: "command".into(),
        image: Some(request.image.clone()),
        command: Some(request.command.clone()),
        args: None,
        env: Some(
            request
                .env
                .iter()
                .map(|(name, value)| EnvVar {
                    name: name.clone(),
                    value: Some(value.clone()),
                    value_from: None,
                })
                .collect(),
        ),
        working_dir: Some(request.workspace.container_workdir.clone()),
        volume_mounts: Some(mounts),
        image_pull_policy: Some(request.pull_policy.as_kubernetes_value().into()),
        resources: Some(resources),
        security_context: Some(SecurityContext {
            allow_privilege_escalation: Some(false),
            privileged: Some(false),
            read_only_root_filesystem: Some(request.read_only_rootfs),
            run_as_user: Some(uid),
            run_as_group: Some(gid),
            run_as_non_root: Some(uid != 0),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".into(),
                localhost_profile: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let network_profile = if request.network == "none" {
        "isolated"
    } else {
        request.network.as_str()
    };
    let mut labels = BTreeMap::new();
    labels.insert(JOB_LABEL.to_string(), request.name.clone());
    labels.insert(RUN_LABEL.to_string(), request.name.clone());
    labels.insert(NETWORK_LABEL.to_string(), network_profile.to_string());

    let mut pod_labels = labels.clone();
    pod_labels.insert("autonomics.io/workload".into(), "container-command".into());
    let mut annotations = BTreeMap::new();
    if let Some(pids_limit) = request.pids_limit {
        annotations.insert("autonomics.io/pids-limit".into(), pids_limit.to_string());
    }

    Job {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(config.namespace.clone()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            completions: Some(1),
            parallelism: Some(1),
            active_deadline_seconds: Some(request.timeout_secs as i64),
            ttl_seconds_after_finished: Some(3600),
            selector: Some(LabelSelector {
                match_labels: Some(
                    [(JOB_LABEL.to_string(), request.name.clone())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            }),
            manual_selector: Some(true),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(pod_labels),
                    ..Default::default()
                }),
                spec: Some(build_pod_spec(config, container, volumes, uid, gid)),
            },
            ..Default::default()
        }),
        status: None,
    }
}

fn build_pod_spec(
    config: &K3sConfig,
    container: Container,
    volumes: Vec<Volume>,
    uid: i64,
    gid: i64,
) -> k8s_openapi::api::core::v1::PodSpec {
    k8s_openapi::api::core::v1::PodSpec {
        restart_policy: Some("Never".into()),
        automount_service_account_token: Some(false),
        enable_service_links: Some(false),
        service_account_name: config.service_account.clone(),
        security_context: Some(PodSecurityContext {
            run_as_user: Some(uid),
            run_as_group: Some(gid),
            fs_group: Some(gid),
            run_as_non_root: Some(uid != 0),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".into(),
                localhost_profile: None,
            }),
            ..Default::default()
        }),
        volumes: Some(volumes),
        containers: vec![container],
        ..Default::default()
    }
}

fn resource_requirements(request: &ContainerRunRequest) -> ResourceRequirements {
    let mut limits = BTreeMap::new();
    let mut requests = BTreeMap::new();
    if let Some(cpus) = request.cpus {
        let cpu = format!("{cpus}");
        requests.insert("cpu".into(), quantity(&cpu));
        limits.insert("cpu".into(), quantity(&cpu));
    }
    if let Some(memory) = &request.memory {
        requests.insert("memory".into(), quantity(memory));
        limits.insert("memory".into(), quantity(memory));
    }
    ResourceRequirements {
        requests: (!requests.is_empty()).then_some(requests),
        limits: (!limits.is_empty()).then_some(limits),
        claims: None,
    }
}

fn quantity(value: &str) -> k8s_openapi::apimachinery::pkg::api::resource::Quantity {
    k8s_openapi::apimachinery::pkg::api::resource::Quantity(value.to_string())
}

fn user_ids(request: &ContainerRunRequest) -> (i64, i64) {
    #[cfg(unix)]
    let default = (unsafe { libc::getuid() as i64 }, unsafe {
        libc::getgid() as i64
    });
    #[cfg(not(unix))]
    let default = (1000, 1000);
    let default_gid = default.1;

    request
        .user
        .as_deref()
        .and_then(|user| {
            let mut parts = user.split(':');
            let uid = parts.next()?.parse::<i64>().ok()?;
            let gid = parts
                .next()
                .and_then(|gid| gid.parse::<i64>().ok())
                .unwrap_or(default_gid);
            Some((uid, gid))
        })
        .unwrap_or(default)
}

fn failed_condition_message(conditions: Option<&[JobCondition]>, logs: &str) -> String {
    let condition = conditions
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|condition| condition.type_ == "Failed" && condition.status == "True")
        })
        .map(|condition| {
            format!(
                "reason={}, message={}",
                condition.reason.as_deref().unwrap_or("Unknown"),
                condition.message.as_deref().unwrap_or("job failed")
            )
        })
        .unwrap_or_else(|| "k3s Job failed".into());
    if logs.trim().is_empty() {
        condition
    } else {
        format!("{condition}\n{logs}")
    }
}

fn truncate_log(logs: &str) -> String {
    if logs.len() <= MAX_CAPTURED_OUTPUT_BYTES {
        logs.to_string()
    } else {
        format!(
            "{}\n[output truncated at {MAX_CAPTURED_OUTPUT_BYTES} bytes]",
            &logs[..MAX_CAPTURED_OUTPUT_BYTES]
        )
    }
}

pub(crate) fn validate_request(request: &ContainerRunRequest) -> Result<(), ContainerRuntimeError> {
    if request.image.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if request.command.is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`command` cannot be empty".into(),
        ));
    }
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if !request.workspace.host_path.is_absolute()
        || !request.workspace.host_path.is_dir()
        || !Path::new(&request.workspace.container_workdir).is_absolute()
        || request.workspace.pvc_sub_path.contains("..")
        || request.workspace.pvc_sub_path.starts_with('/')
    {
        return Err(ContainerRuntimeError::Invalid(
            "workspace must be an existing directory mapped to a safe PVC subPath".into(),
        ));
    }
    if !matches!(
        request.network.as_str(),
        "isolated" | "none" | "cluster" | "egress"
    ) {
        return Err(ContainerRuntimeError::Invalid(format!(
            "unsupported network profile `{}`",
            request.network
        )));
    }
    if let Some(cpus) = request.cpus
        && cpus <= 0.0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = request.pids_limit
        && pids_limit <= 0
    {
        return Err(ContainerRuntimeError::Invalid(
            "`pids_limit` must be positive".into(),
        ));
    }
    if let Some(user) = &request.user {
        let mut parts = user.split(':');
        let uid_valid = parts.next().is_some_and(|uid| uid.parse::<i64>().is_ok());
        let gid = parts.next();
        let gid_valid = match gid {
            Some(gid) => gid.parse::<i64>().is_ok(),
            None => true,
        };
        if !uid_valid || !gid_valid || parts.next().is_some() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`user` must be `uid:gid` or `uid`, got `{user}`"
            )));
        }
    }
    for panel in &request.panels {
        panel
            .host_path
            .file_name()
            .ok_or_else(|| ContainerRuntimeError::Invalid("invalid panel cache path".into()))?;
        if panel.pvc_sub_path.contains("..") || panel.pvc_sub_path.starts_with('/') {
            return Err(ContainerRuntimeError::Invalid(format!(
                "panel `{}` has an unsafe PVC subPath",
                panel.id
            )));
        }
    }
    Ok(())
}

pub fn workspace_ref(
    workspace_root: &Path,
    host_path: &Path,
    container_workdir: &str,
) -> Result<WorkspaceRef, ContainerRuntimeError> {
    let relative = host_path.strip_prefix(workspace_root).map_err(|_| {
        ContainerRuntimeError::Invalid(format!(
            "workspace `{}` is outside k3s workspace root `{}`",
            host_path.display(),
            workspace_root.display()
        ))
    })?;
    let pvc_sub_path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if pvc_sub_path.is_empty() || pvc_sub_path.contains("..") || pvc_sub_path.starts_with('/') {
        return Err(ContainerRuntimeError::Invalid(
            "workspace cannot map to the PVC root".into(),
        ));
    }
    Ok(WorkspaceRef {
        host_path: host_path.to_path_buf(),
        pvc_sub_path,
        container_workdir: container_workdir.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CachedPanel, PullPolicy};

    fn request(network: &str) -> ContainerRunRequest {
        let workspace = WorkspaceRef {
            host_path: Path::new("/workspace/runs/abc").into(),
            pvc_sub_path: "runs/abc".into(),
            container_workdir: "/work".into(),
        };
        ContainerRunRequest {
            image: "quay.io/example/tool@sha256:abcdef".into(),
            command: vec!["tool".into(), "--panel".into(), "/panels/1000g".into()],
            workspace,
            env: vec![("AUTONOMICS_WORKDIR".into(), "/work".into())],
            panels: vec![CachedPanel {
                id: "1000g".into(),
                digest: format!("sha256:{}", "1".repeat(64)),
                host_path: PathBuf::from("/panels/cache/1000g@digest"),
                pvc_sub_path: "panels/1000g@digest".into(),
                mount_path: "/panels/1000g".into(),
            }],
            network: network.into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(2.5),
            memory: Some("8Gi".into()),
            pids_limit: Some(512),
            shm_size: Some("1Gi".into()),
            user: Some("1000:1000".into()),
            timeout_secs: 60,
            name: "test-job".into(),
        }
    }

    fn config() -> K3sConfig {
        K3sConfig {
            namespace: "autonomics".into(),
            context: None,
            workspace_pvc: "workspace-pvc".into(),
            workspace_root: PathBuf::from("/workspace"),
            panel_pvc: "panel-pvc".into(),
            panel_cache_root: PathBuf::from("/panels"),
            panel_pvc_prefix: String::new(),
            service_account: Some("autonomics".into()),
            poll_interval_ms: 10,
        }
    }

    #[test]
    fn job_uses_single_ephemeral_pod_and_read_only_panels() {
        let job = build_job(&request("isolated"), &config());
        let spec = job.spec.unwrap();
        assert_eq!(spec.backoff_limit, Some(0));
        assert_eq!(spec.completions, Some(1));
        assert_eq!(spec.parallelism, Some(1));

        let pod = spec.template.spec.unwrap();
        assert_eq!(pod.restart_policy.as_deref(), Some("Never"));
        assert_eq!(pod.automount_service_account_token, Some(false));
        let container = &pod.containers[0];
        assert_eq!(
            container.image.as_deref(),
            Some("quay.io/example/tool@sha256:abcdef")
        );
        assert_eq!(container.image_pull_policy.as_deref(), Some("Never"));
        assert_eq!(
            container
                .security_context
                .as_ref()
                .unwrap()
                .read_only_root_filesystem,
            Some(true)
        );
        assert_eq!(
            container
                .security_context
                .as_ref()
                .unwrap()
                .allow_privilege_escalation,
            Some(false)
        );
        let mounts = container.volume_mounts.as_ref().unwrap();
        assert!(
            mounts
                .iter()
                .any(|mount| mount.mount_path == "/panels/1000g" && mount.read_only == Some(true))
        );
        assert!(
            pod.volumes
                .as_ref()
                .unwrap()
                .iter()
                .all(|volume| volume.host_path.is_none())
        );
    }

    #[test]
    fn legacy_none_maps_to_isolated_profile_and_remains_valid() {
        let mut valid = request("none");
        let workspace = tempfile::tempdir().unwrap();
        valid.workspace.host_path = workspace.path().to_path_buf();
        valid.workspace.pvc_sub_path = "run".into();
        assert!(validate_request(&valid).is_ok());
        let job = build_job(&request("none"), &config());
        let labels = job.metadata.labels.unwrap();
        assert_eq!(
            labels.get(NETWORK_LABEL).map(String::as_str),
            Some("isolated")
        );
    }

    #[test]
    fn workspace_must_be_inside_configured_root() {
        let ok = workspace_ref(
            Path::new("/workspace"),
            Path::new("/workspace/runs/x"),
            "/work",
        )
        .unwrap();
        assert_eq!(ok.pvc_sub_path, "runs/x");
        assert!(workspace_ref(Path::new("/workspace"), Path::new("/tmp/x"), "/work").is_err());
    }
}
