//! Slurm (HPC) integration settings.
//!
//! When enabled, `process` tasks route to the cluster as foreground
//! sbatch jobs over ssh; every other task keeps running on the local
//! executor. Credentials stay in the operator's ssh setup (the transport
//! forces `BatchMode`) — nothing here carries secrets. Every field has an
//! `AUTONOMICS_SLURM_*` env override for legacy deployment and first enrollment.
//! Once an ordinary compute catalog exists, its selected immutable target
//! revision controls placement. Operator API changes affect new runs;
//! environment changes cannot retarget historical work.

use std::path::PathBuf;

/// Env var enabling the Slurm (HPC) task executor: `process` tasks are
/// then routed to the cluster as sbatch jobs.
pub const ENV_SLURM_ENABLED: &str = "AUTONOMICS_SLURM_ENABLED";
/// Coordinator-side JSON manifest binding full OCI references to compute-node SIFs.
pub const ENV_SLURM_APPTAINER_MANIFEST: &str = "AUTONOMICS_SLURM_APPTAINER_MANIFEST";
/// Enable account filesystem, resource discovery and explicit Slurm agent tools.
pub const ENV_HPC_ENABLED: &str = "AUTONOMICS_HPC_ENABLED";
pub const ENV_HPC_DATA_SSH_HOST: &str = "AUTONOMICS_HPC_DATA_SSH_HOST";
pub const ENV_HPC_PI_SSH_HOST: &str = "AUTONOMICS_HPC_PI_SSH_HOST";
pub const ENV_HPC_REMOTE_PYTHON: &str = "AUTONOMICS_HPC_REMOTE_PYTHON";
pub const ENV_SLURM_ACCOUNT: &str = "AUTONOMICS_SLURM_ACCOUNT";

/// Env var overriding the Slurm login-node ssh destination (an
/// `~/.ssh/config` alias or `user@host`).
pub const ENV_SLURM_SSH_HOST: &str = "AUTONOMICS_SLURM_SSH_HOST";

/// Env var overriding the ssh program used to reach the Slurm login node.
pub const ENV_SLURM_SSH_COMMAND: &str = "AUTONOMICS_SLURM_SSH_COMMAND";

/// Env var overriding the remote scratch root for Slurm task workspaces.
pub const ENV_SLURM_SCRATCH_DIR: &str = "AUTONOMICS_SLURM_SCRATCH_DIR";

/// Env var overriding the local download root for Slurm artifacts.
pub const ENV_SLURM_ARTIFACT_ROOT: &str = "AUTONOMICS_SLURM_ARTIFACT_ROOT";

/// Operator-owned JSON placement profile for typed remote workers.
pub const ENV_SLURM_TYPED_WORKER_CONFIG: &str = "AUTONOMICS_SLURM_TYPED_WORKER_CONFIG";

/// Env var overriding the default Slurm partition.
pub const ENV_SLURM_PARTITION: &str = "AUTONOMICS_SLURM_PARTITION";
pub const ENV_SLURM_GPU_PARTITION: &str = "AUTONOMICS_SLURM_GPU_PARTITION";
pub const ENV_SLURM_GPU_CPUS_PER_GPU: &str = "AUTONOMICS_SLURM_GPU_CPUS_PER_GPU";
pub const ENV_SLURM_GPU_QOS: &str = "AUTONOMICS_SLURM_GPU_QOS";

/// Env var overriding the default per-CPU memory sbatch directive.
pub const ENV_SLURM_MEM_PER_CPU: &str = "AUTONOMICS_SLURM_MEM_PER_CPU";

/// Env var overriding the default sbatch wall-clock limit.
pub const ENV_SLURM_DEFAULT_TIME: &str = "AUTONOMICS_SLURM_DEFAULT_TIME";

/// Env var overriding the Slurm terminal-state polling cadence (seconds).
pub const ENV_SLURM_POLL_INTERVAL_SECS: &str = "AUTONOMICS_SLURM_POLL_INTERVAL_SECS";

/// Env var overriding the ssh connect timeout (seconds).
pub const ENV_SLURM_SSH_CONNECT_TIMEOUT_SECS: &str = "AUTONOMICS_SLURM_SSH_CONNECT_TIMEOUT_SECS";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SlurmConfig {
    /// Persistent compute-side content-addressed cache, outside per-job scratch.
    #[serde(default = "default_slurm_panel_cache")]
    pub panel_cache_remote_dir: String,
    /// Operator-owned local SIF manifest; containers require this explicit profile.
    #[serde(default = "default_slurm_apptainer_manifest")]
    pub apptainer_manifest: Option<PathBuf>,
    /// Register user-authorized account tools independently of DAG routing.
    #[serde(default = "default_hpc_enabled")]
    pub tools_enabled: bool,
    /// Dedicated data-transfer endpoint; never infer that its home equals login home.
    #[serde(default = "default_hpc_data_ssh_host")]
    pub data_ssh_host: Option<String>,
    /// Optional Pi/Lustre login profile; data endpoint may expose both filesystems.
    #[serde(default = "default_hpc_pi_ssh_host")]
    pub pi_ssh_host: Option<String>,
    #[serde(default = "default_hpc_remote_python")]
    pub remote_python: String,
    #[serde(default = "default_slurm_account")]
    pub account: Option<String>,
    /// Route `process` tasks to the cluster.
    #[serde(default = "default_slurm_enabled")]
    pub enabled: bool,
    /// ssh destination for the cluster login node.
    #[serde(default = "default_slurm_ssh_host")]
    pub ssh_host: String,
    /// ssh program to invoke (tests point this at a fake).
    #[serde(default = "default_slurm_ssh_command")]
    pub ssh_command: String,
    /// Remote scratch root for task workspaces; a leading `~/` expands to
    /// the remote home directory.
    #[serde(default = "default_slurm_scratch_remote_dir")]
    pub scratch_remote_dir: String,
    /// Local directory downloaded artifacts land in. Empty (the default)
    /// means `state_dir/slurm-artifacts`, resolved when the executor is
    /// wired — a serde default cannot see the sibling `state_dir`.
    #[serde(default)]
    pub artifact_root: PathBuf,
    /// Optional pinned typed-worker placement profile; absent preserves process-only routing.
    #[serde(default = "default_slurm_typed_worker_config")]
    pub typed_worker_config: Option<PathBuf>,
    /// Default sbatch partition.
    #[serde(default = "default_slurm_partition")]
    pub partition: String,
    /// Operator partition for GPU container requests, separate from CPU routing.
    #[serde(default = "default_slurm_gpu_partition")]
    pub gpu_partition: String,
    /// Minimum allocated CPU cores per GPU, default 16 for the site a100 profile.
    #[serde(default = "default_slurm_gpu_cpus_per_gpu")]
    pub gpu_cpus_per_gpu: u32,
    /// Optional GPU QOS; debuga100 defaults to debug at runtime admission.
    #[serde(default = "default_slurm_gpu_qos")]
    pub gpu_qos: Option<String>,
    /// Memory per allocated CPU when a task requests no explicit total.
    #[serde(default = "default_slurm_mem_per_cpu")]
    pub mem_per_cpu: String,
    /// Wall-clock limit when a task requests no explicit duration.
    #[serde(default = "default_slurm_default_time")]
    pub default_time: String,
    /// Terminal-state polling cadence in seconds.
    #[serde(default = "default_slurm_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// ssh connect timeout in seconds.
    #[serde(default = "default_slurm_ssh_connect_timeout_secs")]
    pub ssh_connect_timeout_secs: u64,
}

impl Default for SlurmConfig {
    fn default() -> Self {
        Self {
            panel_cache_remote_dir: default_slurm_panel_cache(),
            apptainer_manifest: default_slurm_apptainer_manifest(),
            tools_enabled: default_hpc_enabled(),
            data_ssh_host: default_hpc_data_ssh_host(),
            pi_ssh_host: default_hpc_pi_ssh_host(),
            remote_python: default_hpc_remote_python(),
            account: default_slurm_account(),
            enabled: default_slurm_enabled(),
            ssh_host: default_slurm_ssh_host(),
            ssh_command: default_slurm_ssh_command(),
            scratch_remote_dir: default_slurm_scratch_remote_dir(),
            artifact_root: PathBuf::new(),
            typed_worker_config: default_slurm_typed_worker_config(),
            partition: default_slurm_partition(),
            gpu_partition: default_slurm_gpu_partition(),
            gpu_cpus_per_gpu: default_slurm_gpu_cpus_per_gpu(),
            gpu_qos: default_slurm_gpu_qos(),
            mem_per_cpu: default_slurm_mem_per_cpu(),
            default_time: default_slurm_default_time(),
            poll_interval_secs: default_slurm_poll_interval_secs(),
            ssh_connect_timeout_secs: default_slurm_ssh_connect_timeout_secs(),
        }
    }
}

fn slurm_env(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn parse_env_flag(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn default_slurm_enabled() -> bool {
    slurm_env(ENV_SLURM_ENABLED)
        .and_then(|value| parse_env_flag(&value))
        .unwrap_or(false)
}

fn default_slurm_apptainer_manifest() -> Option<PathBuf> {
    slurm_env(ENV_SLURM_APPTAINER_MANIFEST).map(PathBuf::from)
}

fn default_slurm_gpu_cpus_per_gpu() -> u32 {
    slurm_env(ENV_SLURM_GPU_CPUS_PER_GPU)
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(16)
}

fn default_slurm_gpu_qos() -> Option<String> {
    slurm_env(ENV_SLURM_GPU_QOS)
}

fn default_slurm_gpu_partition() -> String {
    slurm_env(ENV_SLURM_GPU_PARTITION).unwrap_or_else(|| "a100".into())
}

fn default_slurm_panel_cache() -> String {
    slurm_env("AUTONOMICS_SLURM_PANEL_CACHE_DIR")
        .unwrap_or_else(|| "~/.cache/autonomics-apptainer-panels".into())
}

fn default_hpc_enabled() -> bool {
    slurm_env(ENV_HPC_ENABLED)
        .and_then(|value| parse_env_flag(&value))
        .unwrap_or(false)
}

fn default_hpc_data_ssh_host() -> Option<String> {
    slurm_env(ENV_HPC_DATA_SSH_HOST)
}

fn default_hpc_pi_ssh_host() -> Option<String> {
    slurm_env(ENV_HPC_PI_SSH_HOST)
}

fn default_hpc_remote_python() -> String {
    slurm_env(ENV_HPC_REMOTE_PYTHON).unwrap_or_else(|| "python3".into())
}

fn default_slurm_account() -> Option<String> {
    slurm_env(ENV_SLURM_ACCOUNT)
}

fn default_slurm_ssh_host() -> String {
    slurm_env(ENV_SLURM_SSH_HOST).unwrap_or_else(|| "sjtu".into())
}

fn default_slurm_ssh_command() -> String {
    slurm_env(ENV_SLURM_SSH_COMMAND).unwrap_or_else(|| "ssh".into())
}

fn default_slurm_scratch_remote_dir() -> String {
    slurm_env(ENV_SLURM_SCRATCH_DIR).unwrap_or_else(|| "~/autonomics-scratch/autonomics".into())
}

fn default_slurm_typed_worker_config() -> Option<PathBuf> {
    slurm_env(ENV_SLURM_TYPED_WORKER_CONFIG).map(PathBuf::from)
}

fn default_slurm_partition() -> String {
    slurm_env(ENV_SLURM_PARTITION).unwrap_or_else(|| "64c512g".into())
}

fn default_slurm_mem_per_cpu() -> String {
    slurm_env(ENV_SLURM_MEM_PER_CPU).unwrap_or_else(|| "8G".into())
}

fn default_slurm_default_time() -> String {
    slurm_env(ENV_SLURM_DEFAULT_TIME).unwrap_or_else(|| "01:00:00".into())
}

fn default_slurm_poll_interval_secs() -> u64 {
    slurm_env(ENV_SLURM_POLL_INTERVAL_SECS)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(15)
}

fn default_slurm_ssh_connect_timeout_secs() -> u64 {
    slurm_env(ENV_SLURM_SSH_CONNECT_TIMEOUT_SECS)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(10)
}
