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

// ── Operator site profile ────────────────────────────────────────────────
//
// The panel-edited persistent layer between env bootstrap and code
// defaults. `<state_dir>/slurm-site.json`; every field optional. Precedence
// at daemon start: enrolled compute catalog > site profile > env > default.
// Writes go through `PUT /hpc/config` (atomic staging+rename); a running
// daemon never re-reads the file — the panel surfaces a restart hint
// instead, because ssh targets and executors are wired once at boot.

/// Operator-editable site profile persisted at
/// [`site_profile_path`](Self::path). `None` fields fall through to env /
/// built-in defaults; `deny_unknown_fields` keeps typos from silently
/// disabling a setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SlurmSiteProfile {
    pub enabled: Option<bool>,
    pub tools_enabled: Option<bool>,
    pub ssh_host: Option<String>,
    pub ssh_command: Option<String>,
    pub scratch_remote_dir: Option<String>,
    pub artifact_root: Option<PathBuf>,
    pub apptainer_manifest: Option<PathBuf>,
    pub typed_worker_config: Option<PathBuf>,
    pub panel_cache_remote_dir: Option<String>,
    pub account: Option<String>,
    pub partition: Option<String>,
    pub gpu_partition: Option<String>,
    pub gpu_cpus_per_gpu: Option<u32>,
    pub gpu_qos: Option<String>,
    pub mem_per_cpu: Option<String>,
    pub default_time: Option<String>,
    pub poll_interval_secs: Option<u64>,
    pub ssh_connect_timeout_secs: Option<u64>,
    pub data_ssh_host: Option<String>,
    pub pi_ssh_host: Option<String>,
    pub remote_python: Option<String>,
}

impl SlurmSiteProfile {
    /// `<state_dir>/slurm-site.json`.
    pub fn path(state_dir: &std::path::Path) -> PathBuf {
        state_dir.join("slurm-site.json")
    }

    /// Load the profile; `None` when the file does not exist. Unreadable or
    /// malformed profiles surface as errors — a silent fallback would let a
    /// typo disable production routing.
    pub fn load(state_dir: &std::path::Path) -> Result<Option<Self>, String> {
        let path = Self::path(state_dir);
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {}: {e}", path.display())),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("parse {}: {e}", path.display())),
        }
    }

    /// Overlay every present field onto `config`.
    pub fn apply_to(&self, config: &mut SlurmConfig) {
        if let Some(v) = self.enabled {
            config.enabled = v;
        }
        if let Some(v) = self.tools_enabled {
            config.tools_enabled = v;
        }
        if let Some(v) = &self.ssh_host {
            config.ssh_host = v.clone();
        }
        if let Some(v) = &self.ssh_command {
            config.ssh_command = v.clone();
        }
        if let Some(v) = &self.scratch_remote_dir {
            config.scratch_remote_dir = v.clone();
        }
        if let Some(v) = &self.artifact_root {
            config.artifact_root = v.clone();
        }
        if let Some(v) = &self.apptainer_manifest {
            config.apptainer_manifest = Some(v.clone());
        }
        if let Some(v) = &self.typed_worker_config {
            config.typed_worker_config = Some(v.clone());
        }
        if let Some(v) = &self.panel_cache_remote_dir {
            config.panel_cache_remote_dir = v.clone();
        }
        if let Some(v) = &self.account {
            config.account = Some(v.clone());
        }
        if let Some(v) = &self.partition {
            config.partition = v.clone();
        }
        if let Some(v) = &self.gpu_partition {
            config.gpu_partition = v.clone();
        }
        if let Some(v) = self.gpu_cpus_per_gpu {
            config.gpu_cpus_per_gpu = v;
        }
        if let Some(v) = &self.gpu_qos {
            config.gpu_qos = Some(v.clone());
        }
        if let Some(v) = &self.mem_per_cpu {
            config.mem_per_cpu = v.clone();
        }
        if let Some(v) = &self.default_time {
            config.default_time = v.clone();
        }
        if let Some(v) = self.poll_interval_secs {
            config.poll_interval_secs = v;
        }
        if let Some(v) = self.ssh_connect_timeout_secs {
            config.ssh_connect_timeout_secs = v;
        }
        if let Some(v) = &self.data_ssh_host {
            config.data_ssh_host = Some(v.clone());
        }
        if let Some(v) = &self.pi_ssh_host {
            config.pi_ssh_host = Some(v.clone());
        }
        if let Some(v) = &self.remote_python {
            config.remote_python = v.clone();
        }
    }

    /// Persist atomically: stage alongside the target, then rename over it.
    /// The site directory is created on demand.
    pub fn store(&self, state_dir: &std::path::Path) -> Result<PathBuf, String> {
        let path = Self::path(state_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let stage = path.with_extension("json.staging");
        std::fs::write(&stage, &bytes).map_err(|e| format!("write {}: {e}", stage.display()))?;
        std::fs::rename(&stage, &path).map_err(|e| format!("rename into place: {e}"))?;
        Ok(path)
    }

    /// Operator-input validation before a write is accepted. Only rejects
    /// values the daemon would refuse at boot; filesystem reachability is
    /// deliberately not checked here (paths may be created later).
    pub fn validate(&self) -> Result<(), String> {
        let non_empty = |field: &str, value: &str| {
            (!value.trim().is_empty())
                .then_some(())
                .ok_or_else(|| format!("{field} must not be empty"))
        };
        if let Some(v) = &self.ssh_host {
            non_empty("ssh_host", v)?;
        }
        if let Some(v) = &self.ssh_command {
            non_empty("ssh_command", v)?;
        }
        if let Some(v) = &self.scratch_remote_dir {
            non_empty("scratch_remote_dir", v)?;
            if v.contains(char::is_whitespace) {
                return Err("scratch_remote_dir must not contain whitespace (sbatch \
                            directive values cannot be quoted)"
                    .into());
            }
        }
        if let Some(v) = &self.partition {
            non_empty("partition", v)?;
            if v.contains(char::is_whitespace) {
                return Err("partition must not contain whitespace".into());
            }
        }
        if let Some(v) = &self.gpu_partition {
            non_empty("gpu_partition", v)?;
        }
        if let Some(v) = self.gpu_cpus_per_gpu {
            if v == 0 {
                return Err("gpu_cpus_per_gpu must be at least 1".into());
            }
        }
        if let Some(v) = self.poll_interval_secs {
            if v == 0 {
                return Err("poll_interval_secs must be at least 1".into());
            }
        }
        if let Some(v) = self.ssh_connect_timeout_secs {
            if v == 0 {
                return Err("ssh_connect_timeout_secs must be at least 1".into());
            }
        }
        Ok(())
    }
}

/// Parse a Slurm-style walltime (`HH:MM:SS` or `D-HH:MM:SS`) into a
/// duration. The direct SSH backend reuses the operator's familiar
/// `default_time` spelling as its task horizon.
pub fn parse_walltime(value: &str) -> Option<std::time::Duration> {
    let (days, rest) = match value.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, value),
    };
    let parts: Vec<_> = rest.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: u64 = parts[0].parse().ok()?;
    let minutes: u64 = parts[1].parse().ok()?;
    let seconds: u64 = parts[2].parse().ok()?;
    if minutes > 59 || seconds > 59 {
        return None;
    }
    Some(std::time::Duration::from_secs(
        days * 86_400 + hours * 3_600 + minutes * 60 + seconds,
    ))
}

#[cfg(test)]
mod site_profile_tests {
    use super::*;

    #[test]
    fn round_trips_through_disk_and_applies_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let profile = SlurmSiteProfile {
            enabled: Some(true),
            ssh_host: Some("hpc-login".into()),
            poll_interval_secs: Some(30),
            account: Some("proj42".into()),
            ..Default::default()
        };
        let path = profile.store(dir.path()).unwrap();
        assert_eq!(path, SlurmSiteProfile::path(dir.path()));
        assert!(path.with_extension("json.staging").parent().is_some());
        assert!(
            !SlurmSiteProfile::path(dir.path())
                .to_string_lossy()
                .contains("staging")
        );

        let loaded = SlurmSiteProfile::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, profile);

        let mut config = SlurmConfig::default();
        loaded.apply_to(&mut config);
        assert!(config.enabled);
        assert_eq!(config.ssh_host, "hpc-login");
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.account.as_deref(), Some("proj42"));
        // Untouched fields keep their env/default resolution.
        assert_eq!(config.partition, SlurmConfig::default().partition);
    }

    #[test]
    fn absent_profile_loads_as_none_and_errors_are_surfaced() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SlurmSiteProfile::load(dir.path()).unwrap().is_none());

        std::fs::write(SlurmSiteProfile::path(dir.path()), b"{not json").unwrap();
        assert!(
            SlurmSiteProfile::load(dir.path()).is_err(),
            "malformed profile must error"
        );

        std::fs::write(
            SlurmSiteProfile::path(dir.path()),
            br#"{"ssh_host_but_typo": 1}"#,
        )
        .unwrap();
        assert!(
            SlurmSiteProfile::load(dir.path()).is_err(),
            "unknown fields must be rejected (deny_unknown_fields)"
        );
    }

    #[test]
    fn validate_rejects_operator_errors() {
        assert!(
            SlurmSiteProfile {
                ssh_host: Some("  ".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SlurmSiteProfile {
                scratch_remote_dir: Some("~/no spaces/here".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SlurmSiteProfile {
                poll_interval_secs: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SlurmSiteProfile {
                gpu_cpus_per_gpu: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        // A minimal sane profile passes.
        assert!(
            SlurmSiteProfile {
                enabled: Some(true),
                ssh_host: Some("sjtu".into()),
                partition: Some("64c512g".into()),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn startup_config_applies_the_site_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = SlurmSiteProfile {
            ssh_host: Some("boot-time-host".into()),
            ..Default::default()
        };
        profile.store(dir.path()).unwrap();

        let mut config = crate::RuntimeConfig::default();
        config.state_dir = dir.path().to_path_buf();
        let resolved = crate::compute::startup_config(&config).unwrap();
        assert_eq!(resolved.slurm.ssh_host, "boot-time-host");

        // No catalog present — the site profile is the only overlay.
        std::fs::remove_file(SlurmSiteProfile::path(dir.path())).unwrap();
        let resolved = crate::compute::startup_config(&config).unwrap();
        assert_eq!(resolved.slurm.ssh_host, SlurmConfig::default().ssh_host);
    }
}
