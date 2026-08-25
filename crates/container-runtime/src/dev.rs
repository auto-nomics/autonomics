//! Persistent development workspaces running as ordinary Kubernetes Pods.
//!
//! Unlike `container_command`, these Pods intentionally stay alive. Source and
//! other workspace files live on the existing workspace PVC; image creation is
//! an explicit, reproducible build from that workspace.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use k8s_openapi::api::core::v1::{
    Container, EnvVar, PersistentVolumeClaimVolumeSource, Pod, PodSecurityContext,
    ResourceRequirements, SeccompProfile, SecurityContext, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;

use crate::error::ContainerRuntimeError;
use crate::k3s::K3sRuntime;
use crate::runtime::{ContainerRuntime, MAX_CAPTURED_OUTPUT_BYTES};
use crate::types::{
    ContainerRunRequest, DevExecRequest, DevExecResult, DevImageBuildRequest, DevImageBuildResult,
    DevWorkspaceCreate, DevWorkspaceStatus, PullPolicy, WorkspaceRef,
};

pub const DEV_CONTAINER_NAME: &str = "dev";
pub const DEV_CONTAINER_WORKDIR: &str = "/workspace";
const DEV_WORKLOAD_LABEL: &str = "autonomics.io/workload";
const DEV_ID_LABEL: &str = "autonomics.io/dev-workspace";
const NETWORK_LABEL: &str = "autonomics.io/network";
const IMAGE_ANNOTATION: &str = "autonomics.io/image";

impl K3sRuntime {
    /// Create or attach to a persistent development workspace.
    pub async fn create_dev_workspace(
        &self,
        request: DevWorkspaceCreate,
    ) -> Result<DevWorkspaceStatus, ContainerRuntimeError> {
        validate_create(&request)?;
        let config = self.config();
        let host_path = dev_workspace_path(config, &request.id)?;
        std::fs::create_dir_all(&host_path)?;
        let metadata_path = host_path.join(".autonomics/workspace.json");
        if metadata_path.is_file() {
            let metadata = read_workspace_metadata(&host_path)?;
            if metadata.image != request.image {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "workspace `{}` is already associated with image `{}`",
                    request.id, metadata.image
                )));
            }
        } else {
            write_workspace_metadata(&host_path, &request)?;
        }

        let pod_name = dev_pod_name(&request.id);
        let pod = build_dev_pod(&request, config);
        let pods: Api<Pod> = Api::namespaced(self.client().await?.clone(), &config.namespace);
        match pods.create(&PostParams::default(), &pod).await {
            Ok(_) => {}
            Err(kube::Error::Api(error)) if error.is_conflict() => {
                let existing = pods
                    .get(&pod_name)
                    .await
                    .map_err(ContainerRuntimeError::Kubernetes)?;
                let annotations = existing.metadata.annotations.unwrap_or_default();
                let existing_image = annotations
                    .get(IMAGE_ANNOTATION)
                    .map(String::as_str)
                    .unwrap_or("?");
                if existing_image != request.image {
                    return Err(ContainerRuntimeError::Invalid(format!(
                        "workspace `{}` already exists with image `{existing_image}`",
                        request.id
                    )));
                }
            }
            Err(error) => return Err(error.into()),
        }

        self.wait_dev_workspace(&request.id, request.timeout_secs)
            .await
    }

    pub async fn dev_workspace_status(
        &self,
        id: &str,
    ) -> Result<DevWorkspaceStatus, ContainerRuntimeError> {
        validate_id(id)?;
        self.pod_status(id).await
    }

    pub async fn list_dev_workspaces(
        &self,
    ) -> Result<Vec<DevWorkspaceStatus>, ContainerRuntimeError> {
        let config = self.config();
        let pods: Api<Pod> = Api::namespaced(self.client().await?.clone(), &config.namespace);
        let list = pods
            .list(&ListParams::default().labels(&format!("{DEV_WORKLOAD_LABEL}=container-dev")))
            .await?;
        Ok(list
            .items
            .iter()
            .filter_map(|pod| pod_to_status(pod, config))
            .collect())
    }

    pub async fn exec_in_dev_workspace(
        &self,
        request: DevExecRequest,
    ) -> Result<DevExecResult, ContainerRuntimeError> {
        validate_id(&request.workspace_id)?;
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

        let status = self.pod_status(&request.workspace_id).await?;
        if status.phase != "Running" || !status.ready {
            return Err(ContainerRuntimeError::Invalid(format!(
                "workspace `{}` is not ready (phase={}, ready={})",
                request.workspace_id, status.phase, status.ready
            )));
        }

        let pods: Api<Pod> =
            Api::namespaced(self.client().await?.clone(), &self.config().namespace);
        let command = with_workdir(request.workdir.as_deref(), request.command)?;
        let attach = kube::api::AttachParams::default()
            .container(DEV_CONTAINER_NAME)
            .stdin(false)
            .stdout(true)
            .stderr(true)
            .tty(false);
        let mut attached = pods
            .exec(&dev_pod_name(&request.workspace_id), command, &attach)
            .await?;
        let status_future = attached.take_status();
        let stdout_reader = attached
            .stdout()
            .ok_or_else(|| ContainerRuntimeError::Invalid("exec did not attach stdout".into()))?;
        let stderr_reader = attached
            .stderr()
            .ok_or_else(|| ContainerRuntimeError::Invalid("exec did not attach stderr".into()))?;
        let stdout_task = tokio::spawn(read_capped(stdout_reader, MAX_CAPTURED_OUTPUT_BYTES));
        let stderr_task = tokio::spawn(read_capped(stderr_reader, MAX_CAPTURED_OUTPUT_BYTES));

        let joined =
            tokio::time::timeout(Duration::from_secs(request.timeout_secs), attached.join()).await;
        if joined.is_err() {
            return Err(ContainerRuntimeError::Timeout {
                timeout_secs: request.timeout_secs,
            });
        }
        match joined {
            Ok(result) => result.map_err(|error| {
                ContainerRuntimeError::Invalid(format!("container exec stream failed: {error}"))
            })?,
            Err(_) => unreachable!("timeout branch already returned"),
        };

        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let remote_status = match status_future {
            Some(status_future) => status_future.await,
            None => None,
        };
        let failed = remote_status
            .as_ref()
            .is_some_and(|status| status.status.as_deref() == Some("Failure"));
        Ok(DevExecResult {
            exit_code: remote_exit_code(remote_status.as_ref()).unwrap_or(i32::from(failed)),
            stdout: decode_captured(&stdout),
            stderr: decode_captured(&stderr),
        })
    }

    /// Stop a development Pod while retaining its PVC workspace.
    pub async fn stop_dev_workspace(&self, id: &str) -> Result<(), ContainerRuntimeError> {
        validate_id(id)?;
        let config = self.config();
        let pods: Api<Pod> = Api::namespaced(self.client().await?.clone(), &config.namespace);
        pods.delete(&dev_pod_name(id), &DeleteParams::default())
            .await?;
        Ok(())
    }

    /// Build an OCI image tar from a persistent workspace with Kaniko.
    ///
    /// This is deliberately not a mutable-container commit. The generated
    /// Dockerfile records the base image and copies the workspace, making the
    /// artifact reproducible from source files.
    pub async fn build_dev_workspace_image(
        &self,
        request: DevImageBuildRequest,
    ) -> Result<DevImageBuildResult, ContainerRuntimeError> {
        validate_id(&request.workspace_id)?;
        if request.timeout_secs == 0 {
            return Err(ContainerRuntimeError::Invalid(
                "`timeout_secs` must be greater than zero".into(),
            ));
        }

        let config = self.config();
        let host_path = dev_workspace_path(config, &request.workspace_id)?;
        if !host_path.is_dir() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "workspace `{}` does not exist",
                request.workspace_id
            )));
        }
        let metadata = read_workspace_metadata(&host_path)?;
        let base_image = request.base_image.unwrap_or(metadata.image);
        let autonomics_dir = host_path.join(".autonomics");
        std::fs::create_dir_all(&autonomics_dir)?;
        let dockerignore = host_path.join(".dockerignore");
        let mut ignored = std::fs::read_to_string(&dockerignore).unwrap_or_default();
        for excluded in [".autonomics/", ".home/", ".cache/"] {
            if !ignored.lines().any(|line| line.trim() == excluded) {
                ignored.push_str(&format!("\n{excluded}\n"));
            }
        }
        std::fs::write(&dockerignore, ignored)?;
        let workspace_dockerfile = host_path.join("Dockerfile");
        let dockerfile = if workspace_dockerfile.is_file() {
            "/workspace/Dockerfile".to_string()
        } else {
            std::fs::write(
                autonomics_dir.join("image.Dockerfile"),
                format!(
                    "ARG BASE_IMAGE\nFROM ${{BASE_IMAGE}}\nWORKDIR {DEV_CONTAINER_WORKDIR}\nCOPY . {DEV_CONTAINER_WORKDIR}\n"
                ),
            )?;
            "/workspace/.autonomics/image.Dockerfile".to_string()
        };

        let run = ContainerRunRequest {
            name: format!("autonomics-image-{}", uuid::Uuid::new_v4().simple()),
            image: request.builder_image,
            command: vec![
                "/kaniko/executor".into(),
                format!("--dockerfile={dockerfile}"),
                "--context=dir:///workspace".into(),
                "--no-push".into(),
                "--tar-path=/workspace/.autonomics/image.tar".into(),
                format!("--destination=autonomics/dev/{}", request.workspace_id),
                format!("--build-arg=BASE_IMAGE={base_image}"),
            ],
            workspace: WorkspaceRef {
                host_path: host_path.clone(),
                pvc_sub_path: format!("dev/{}", request.workspace_id),
                container_workdir: DEV_CONTAINER_WORKDIR.into(),
            },
            env: Vec::new(),
            panels: Vec::new(),
            network: "egress".into(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            user: Some("0:0".into()),
            timeout_secs: request.timeout_secs,
        };
        let result = ContainerRuntime::run(self, run).await?;
        let image_tar = autonomics_dir.join("image.tar");
        if !image_tar.is_file() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "image build completed without `{}`",
                image_tar.display()
            )));
        }
        Ok(DevImageBuildResult {
            workspace_id: request.workspace_id,
            image_tar_host_path: image_tar,
            logs: result.stdout,
        })
    }

    pub async fn wait_dev_workspace(
        &self,
        id: &str,
        timeout_secs: u64,
    ) -> Result<DevWorkspaceStatus, ContainerRuntimeError> {
        validate_id(id)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs.max(1));
        loop {
            let status = self.pod_status(id).await?;
            if status.ready {
                return Ok(status);
            }
            if status.phase == "Failed" || status.phase == "Succeeded" {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "development workspace Pod reached terminal phase `{}`",
                    status.phase
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ContainerRuntimeError::Timeout { timeout_secs });
            }
            tokio::time::sleep(Duration::from_millis(self.config().poll_interval_ms)).await;
        }
    }

    async fn pod_status(&self, id: &str) -> Result<DevWorkspaceStatus, ContainerRuntimeError> {
        let config = self.config();
        let pods: Api<Pod> = Api::namespaced(self.client().await?.clone(), &config.namespace);
        let pod = pods
            .get(&dev_pod_name(id))
            .await
            .map_err(ContainerRuntimeError::Kubernetes)?;
        pod_to_status(&pod, config).ok_or_else(|| {
            ContainerRuntimeError::Invalid(format!(
                "Pod `{}` is not a development workspace",
                pod.metadata.name.unwrap_or_default()
            ))
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WorkspaceMetadata {
    image: String,
}

fn validate_create(request: &DevWorkspaceCreate) -> Result<(), ContainerRuntimeError> {
    validate_id(&request.id)?;
    if request.image.trim().is_empty() {
        return Err(ContainerRuntimeError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if !matches!(request.network.as_str(), "isolated" | "cluster" | "egress") {
        return Err(ContainerRuntimeError::Invalid(format!(
            "unsupported development network profile `{}`",
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
    Ok(())
}

fn validate_id(id: &str) -> Result<(), ContainerRuntimeError> {
    let valid = !id.is_empty()
        && id.len() <= 48
        && id != "-"
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(ContainerRuntimeError::Invalid(
            "`workspace_id` must be 1-48 lowercase DNS label characters ([a-z0-9-])".into(),
        ))
    }
}

fn with_workdir(
    workdir: Option<&str>,
    command: Vec<String>,
) -> Result<Vec<String>, ContainerRuntimeError> {
    let Some(workdir) = workdir else {
        return Ok(command);
    };
    if !std::path::Path::new(workdir).is_absolute() {
        return Err(ContainerRuntimeError::Invalid(
            "`workdir` must be an absolute path".into(),
        ));
    }
    let mut wrapped = vec![
        "/bin/sh".into(),
        "-lc".into(),
        format!("cd -- {workdir:?} && exec \"$@\""),
        command[0].clone(),
    ];
    wrapped.extend(command.into_iter().skip(1));
    Ok(wrapped)
}

fn dev_pod_name(id: &str) -> String {
    format!("autonomics-dev-{id}")
}

fn dev_workspace_path(
    config: &crate::K3sConfig,
    id: &str,
) -> Result<PathBuf, ContainerRuntimeError> {
    let path = config.workspace_root.join("dev").join(id);
    if path.parent() != Some(&config.workspace_root.join("dev"))
        || !path.starts_with(&config.workspace_root)
    {
        return Err(ContainerRuntimeError::Invalid(
            "unsafe workspace path".into(),
        ));
    }
    Ok(path)
}

fn write_workspace_metadata(
    host_path: &std::path::Path,
    request: &DevWorkspaceCreate,
) -> Result<(), ContainerRuntimeError> {
    std::fs::create_dir_all(host_path.join(".autonomics"))?;
    let metadata = WorkspaceMetadata {
        image: request.image.clone(),
    };
    let content = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| ContainerRuntimeError::Invalid(error.to_string()))?;
    std::fs::write(host_path.join(".autonomics/workspace.json"), content)?;
    Ok(())
}

fn read_workspace_metadata(
    host_path: &std::path::Path,
) -> Result<WorkspaceMetadata, ContainerRuntimeError> {
    let path = host_path.join(".autonomics/workspace.json");
    let content = std::fs::read(path)?;
    serde_json::from_slice(&content).map_err(|error| {
        ContainerRuntimeError::Invalid(format!("invalid workspace metadata: {error}"))
    })
}

fn remote_exit_code(
    status: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Status>,
) -> Option<i32> {
    let details = status?.details.as_ref()?;
    details
        .causes
        .iter()
        .flatten()
        .find(|cause| cause.reason.as_deref() == Some("ExitCode"))
        .and_then(|cause| cause.message.as_deref()?.parse::<i32>().ok())
}

fn build_dev_pod(request: &DevWorkspaceCreate, config: &crate::K3sConfig) -> Pod {
    let mut labels = BTreeMap::new();
    labels.insert(DEV_WORKLOAD_LABEL.to_string(), "container-dev".to_string());
    labels.insert(DEV_ID_LABEL.to_string(), request.id.clone());
    labels.insert(NETWORK_LABEL.to_string(), request.network.clone());
    let mut annotations = BTreeMap::new();
    annotations.insert(IMAGE_ANNOTATION.to_string(), request.image.clone());

    let (uid, gid) = control_uid_gid();
    let mut limits = BTreeMap::new();
    let mut resource_requests = BTreeMap::new();
    if let Some(cpus) = request.cpus {
        let value = k8s_openapi::apimachinery::pkg::api::resource::Quantity(cpus.to_string());
        resource_requests.insert("cpu".into(), value.clone());
        limits.insert("cpu".into(), value);
    }
    if let Some(memory) = &request.memory {
        let value = k8s_openapi::apimachinery::pkg::api::resource::Quantity(memory.clone());
        resource_requests.insert("memory".into(), value.clone());
        limits.insert("memory".into(), value);
    }

    Pod {
        metadata: ObjectMeta {
            name: Some(dev_pod_name(&request.id)),
            namespace: Some(config.namespace.clone()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::core::v1::PodSpec {
            restart_policy: Some("Always".into()),
            automount_service_account_token: Some(false),
            enable_service_links: Some(false),
            service_account_name: config.service_account.clone(),
            termination_grace_period_seconds: Some(5),
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
            volumes: Some(vec![
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
                    empty_dir: Some(k8s_openapi::api::core::v1::EmptyDirVolumeSource::default()),
                    ..Default::default()
                },
            ]),
            containers: vec![Container {
                name: DEV_CONTAINER_NAME.into(),
                image: Some(request.image.clone()),
                command: Some(vec![
                    "/bin/sh".into(),
                    "-lc".into(),
                    r#"umask 0002; mkdir -p "$HOME" "$XDG_CACHE_HOME"; exec sleep infinity"#.into(),
                ]),
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
                working_dir: Some(DEV_CONTAINER_WORKDIR.into()),
                volume_mounts: Some(vec![
                    VolumeMount {
                        name: "workspace".into(),
                        mount_path: DEV_CONTAINER_WORKDIR.into(),
                        sub_path: Some(format!("dev/{}", request.id)),
                        read_only: Some(false),
                        ..Default::default()
                    },
                    VolumeMount {
                        name: "tmp".into(),
                        mount_path: "/tmp".into(),
                        read_only: Some(false),
                        ..Default::default()
                    },
                ]),
                image_pull_policy: Some("IfNotPresent".into()),
                resources: Some(ResourceRequirements {
                    requests: (!resource_requests.is_empty()).then_some(resource_requests),
                    limits: (!limits.is_empty()).then_some(limits),
                    claims: None,
                }),
                security_context: Some(SecurityContext {
                    privileged: Some(false),
                    allow_privilege_escalation: Some(false),
                    read_only_root_filesystem: Some(false),
                    seccomp_profile: Some(SeccompProfile {
                        type_: "RuntimeDefault".into(),
                        localhost_profile: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }),
        status: None,
    }
}

fn pod_to_status(pod: &Pod, config: &crate::K3sConfig) -> Option<DevWorkspaceStatus> {
    let labels = pod.metadata.labels.as_ref()?;
    if labels.get(DEV_WORKLOAD_LABEL).map(String::as_str) != Some("container-dev") {
        return None;
    }
    let id = labels.get(DEV_ID_LABEL)?.clone();
    let image = pod
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(IMAGE_ANNOTATION))
        .cloned()
        .unwrap_or_default();
    let phase = pod
        .status
        .as_ref()
        .and_then(|status| status.phase.clone())
        .unwrap_or_else(|| "Pending".into());
    let ready = pod
        .status
        .as_ref()
        .and_then(|status| status.container_statuses.as_ref())
        .is_some_and(|statuses| !statuses.is_empty() && statuses.iter().all(|status| status.ready));
    Some(DevWorkspaceStatus {
        pod_name: pod.metadata.name.clone()?,
        workspace_host_path: config.workspace_root.join("dev").join(&id),
        id,
        namespace: config.namespace.clone(),
        image,
        phase,
        ready,
        container_workdir: DEV_CONTAINER_WORKDIR.into(),
    })
}

fn control_uid_gid() -> (i64, i64) {
    #[cfg(unix)]
    unsafe {
        (libc::getuid() as i64, libc::getgid() as i64)
    }
    #[cfg(not(unix))]
    {
        (1000, 1000)
    }
}

async fn read_capped<R>(mut reader: R, cap: usize) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = vec![0_u8; 8192];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        if output.len() < cap {
            let remaining = cap - output.len();
            output.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    output
}

fn decode_captured(bytes: &[u8]) -> String {
    let value = String::from_utf8_lossy(bytes).into_owned();
    if bytes.len() > MAX_CAPTURED_OUTPUT_BYTES {
        format!("{value}\n[output truncated at {MAX_CAPTURED_OUTPUT_BYTES} bytes]\n")
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn config() -> crate::K3sConfig {
        crate::K3sConfig {
            namespace: "autonomics".into(),
            context: None,
            workspace_pvc: "workspace".into(),
            workspace_root: PathBuf::from("/workspace"),
            panel_pvc: "panels".into(),
            panel_cache_root: PathBuf::from("/panels"),
            panel_pvc_prefix: String::new(),
            service_account: None,
            poll_interval_ms: 10,
            image_builder: "kaniko.example/executor".into(),
        }
    }

    fn create(id: &str, network: &str) -> DevWorkspaceCreate {
        DevWorkspaceCreate {
            id: id.into(),
            image: "docker.io/library/debian:bookworm-slim".into(),
            env: vec![("EDITOR".into(), "vim".into())],
            network: network.into(),
            cpus: Some(2.0),
            memory: Some("4Gi".into()),
            timeout_secs: 300,
        }
    }

    #[test]
    fn dev_pod_is_long_running_and_has_persistent_workspace() {
        let pod = build_dev_pod(&create("demo", "egress"), &config());
        let spec = pod.spec.unwrap();
        assert_eq!(spec.restart_policy.as_deref(), Some("Always"));
        assert_eq!(spec.automount_service_account_token, Some(false));
        let container = &spec.containers[0];
        assert_eq!(
            container.command.as_deref(),
            Some(
                &[
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "umask 0002; mkdir -p \"$HOME\" \"$XDG_CACHE_HOME\"; exec sleep infinity"
                        .to_string()
                ][..]
            )
        );
        assert_eq!(
            container
                .security_context
                .as_ref()
                .unwrap()
                .read_only_root_filesystem,
            Some(false)
        );
        let mount = &container.volume_mounts.as_ref().unwrap()[0];
        assert_eq!(mount.sub_path.as_deref(), Some("dev/demo"));
        assert_eq!(mount.mount_path, DEV_CONTAINER_WORKDIR);
    }

    #[test]
    fn identifiers_and_paths_are_restricted() {
        assert!(validate_id("demo-1").is_ok());
        assert!(validate_id("Demo").is_err());
        assert!(validate_id("../escape").is_err());
        let cfg = config();
        assert_eq!(
            dev_workspace_path(&cfg, "demo").unwrap(),
            Path::new("/workspace/dev/demo")
        );
    }

    #[test]
    fn optional_workdir_wraps_command_without_changing_argv() {
        let wrapped = with_workdir(
            Some("/workspace/src"),
            vec!["cargo".into(), "test".into(), "--".into(), "a b".into()],
        )
        .unwrap();
        assert_eq!(wrapped[0], "/bin/sh");
        assert_eq!(wrapped[1], "-lc");
        assert!(wrapped[2].starts_with("cd -- "));
        assert_eq!(wrapped[3], "cargo");
        assert_eq!(wrapped[4], "test");
        assert_eq!(wrapped[5], "--");
        assert_eq!(wrapped[6], "a b");
    }
}
