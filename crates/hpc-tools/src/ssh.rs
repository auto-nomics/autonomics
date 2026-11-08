//! Bounded, noninteractive SSH JSON transport. Payloads travel on stdin, never
//! as shell fragments or command arguments. Existing host keys are mandatory.

use serde_json::Value;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const BOOTSTRAP: &str = "import json,sys; e=json.load(sys.stdin); exec(compile(e['code'],'<autonomics-hpc-helper>','exec'), {'REQUEST':e['input'],'__name__':'__hpc_remote__'})";

#[derive(Clone, Debug)]
pub struct HpcConfig {
    pub ssh_host: String,
    pub data_ssh_host: Option<String>,
    pub pi_ssh_host: Option<String>,
    pub ssh_command: String,
    pub remote_python: String,
    pub state_dir: PathBuf,
    pub default_partition: String,
    pub default_account: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HpcClient {
    config: HpcConfig,
    endpoints: std::collections::BTreeMap<String, vfs::SshEndpoint>,
}

fn host_valid(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host.len() <= 256
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl HpcClient {
    /// Schema construction only. It cannot contact a host or create state.
    /// Runtime callers must resolve an operator-owned client before execution.
    pub fn definition_only() -> Self {
        Self {
            endpoints: Default::default(),
            config: HpcConfig {
                ssh_host: "unconfigured".into(),
                data_ssh_host: None,
                pi_ssh_host: None,
                ssh_command: "ssh".into(),
                remote_python: "python3".into(),
                state_dir: PathBuf::new(),
                default_partition: String::new(),
                default_account: None,
            },
        }
    }
    pub fn new(config: HpcConfig) -> Result<Self, String> {
        if !host_valid(&config.ssh_host)
            || config
                .data_ssh_host
                .as_deref()
                .is_some_and(|h| !host_valid(h))
            || config
                .pi_ssh_host
                .as_deref()
                .is_some_and(|h| !host_valid(h))
        {
            return Err("SSH host must be a host alias or user@hostname, without options".into());
        }
        if config.ssh_command.is_empty() || config.ssh_command.contains('\0') {
            return Err("SSH command must name one executable".into());
        }
        if config.remote_python.is_empty()
            || !config
                .remote_python
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
        {
            return Err("remote Python must be a simple executable name or absolute path".into());
        }
        if !config.state_dir.is_absolute() {
            return Err("HPC state directory must be absolute".into());
        }
        match fs::DirBuilder::new().mode(0o700).create(&config.state_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(
                    "cannot create private HPC state directory; its parent must already exist"
                        .into(),
                );
            }
        }
        let metadata = fs::symlink_metadata(&config.state_dir)
            .map_err(|_| "cannot inspect HPC state directory".to_string())?;
        // Existing public or foreign state must be explicitly repaired by the
        // operator, not silently adopted or chmod'ed at daemon startup.
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o7777 != 0o700
        {
            return Err(
                "HPC state directory must be an owned non-symlink directory with mode 0700".into(),
            );
        }
        Ok(Self {
            config,
            endpoints: Default::default(),
        })
    }

    pub fn with_endpoints(
        mut self,
        endpoints: std::collections::BTreeMap<String, vfs::SshEndpoint>,
    ) -> Result<Self, String> {
        for endpoint in endpoints.values() {
            endpoint.validate()?;
        }
        self.endpoints = endpoints;
        Ok(self)
    }

    pub fn config(&self) -> &HpcConfig {
        &self.config
    }

    pub fn state_dir(&self) -> &Path {
        &self.config.state_dir
    }

    /// Only operator-configured account endpoints are selectable by an agent.
    /// Pi and Siyuan keep their own home, job state and filesystem namespace.
    pub fn for_cluster(&self, cluster: Option<&str>) -> Result<Self, String> {
        if self.config.state_dir.as_os_str().is_empty() {
            return Err("definition-only HPC client cannot execute".into());
        }
        match cluster {
            None | Some("default" | "siyuan") => Ok(self.clone()),
            Some("pi") => {
                let host = self
                    .config
                    .pi_ssh_host
                    .as_ref()
                    .ok_or("Pi login endpoint is not configured")?;
                let mut config = self.config.clone();
                config.ssh_host = host.clone();
                config.default_partition = "cpu".into();
                Self::new(config)?.with_endpoints(self.endpoints.clone())
            }
            _ => Err("cluster must be default, siyuan or pi".into()),
        }
    }

    pub async fn run_json(&self, code: &str, input: &Value, bulk: bool) -> Result<Value, String> {
        if self.config.state_dir.as_os_str().is_empty() {
            return Err("definition-only HPC client cannot execute".into());
        }
        let payload = serde_json::to_vec(&serde_json::json!({"code":code,"input":input}))
            .map_err(|_| "cannot encode HPC request".to_string())?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err("HPC request exceeds 8 MiB transport limit; use bounded chunks".into());
        }
        let host = if bulk {
            self.config.data_ssh_host.as_ref().ok_or(
                "bulk filesystem operations require an explicitly configured data SSH endpoint",
            )?
        } else {
            &self.config.ssh_host
        };
        let remote_command = format!(
            "{} -B -c {}",
            shell_quote(&self.config.remote_python),
            shell_quote(BOOTSTRAP)
        );
        let mut command = Command::new(&self.config.ssh_command);
        if let Some(endpoint) = self.endpoints.get(host) {
            command.args(endpoint.options());
        }
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
        ]);
        for option in vfs::ssh_channel::options() {
            command.args(["-o", &option]);
        }
        command.args(["--", host, &remote_command]);
        // Vault-backed password/OTP answering, when a vault agent is
        // configured; a no-op otherwise.
        vfs::ssh_channel::attach_credentials(&mut command, host);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| "cannot start SSH transport".to_string())?;
        let mut stdin = child.stdin.take().ok_or("missing SSH stdin")?;
        let stdout = child.stdout.take().ok_or("missing SSH stdout")?;
        let stderr = child.stderr.take().ok_or("missing SSH stderr")?;
        let operation = async {
            let writer = async move {
                stdin.write_all(&payload).await?;
                stdin.shutdown().await?;
                // ChildStdin::shutdown does not itself close the pipe. Own
                // and drop it here so json.load(sys.stdin) observes EOF before
                // we await the remote helper's response or process exit.
                drop(stdin);
                Ok::<_, std::io::Error>(())
            };
            let output = async move {
                let mut bytes = Vec::new();
                let mut pipe = stdout.take((MAX_PAYLOAD_BYTES + 1) as u64);
                pipe.read_to_end(&mut bytes).await?;
                drop(pipe);
                if bytes.len() > MAX_PAYLOAD_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "HPC response limit",
                    ));
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let errors = async move {
                let mut bytes = Vec::new();
                let mut pipe = stderr.take(65_537);
                pipe.read_to_end(&mut bytes).await?;
                drop(pipe);
                if bytes.len() > 65_536 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "HPC stderr limit",
                    ));
                }
                Ok::<_, std::io::Error>(bytes)
            };
            tokio::try_join!(writer, output, errors, child.wait())
        };
        let (_, output, errors, status) = tokio::time::timeout(Duration::from_secs(90), operation)
            .await
            .map_err(|_| {
                "HPC SSH request timed out; a mutation may have completed, inspect before retrying"
                    .to_string()
            })?
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    "HPC SSH response exceeded transport limit".to_string()
                } else {
                    "HPC SSH I/O failed; inspect remote state before retrying a mutation"
                        .to_string()
                }
            })?;
        if output.len() > MAX_PAYLOAD_BYTES || errors.len() > 65_536 {
            return Err("HPC SSH response exceeded transport limit".into());
        }
        if !status.success() {
            // Remote stderr can contain private filenames or authentication
            // details. Do not copy it to persisted agent transcripts.
            return Err(format!(
                "HPC SSH helper failed (exit {:?}); check authentication, host key, remote Python and permissions",
                status.code()
            ));
        }
        serde_json::from_slice(&output).map_err(|_| "HPC SSH helper returned invalid JSON".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ssh_option_and_shell_injection() {
        for host in [
            "-oProxyCommand=bad",
            "user@host;touch /tmp/x",
            "host\nother",
            "",
        ] {
            assert!(!host_valid(host));
        }
        assert!(host_valid("zj-2002@pilogin.hpc.sjtu.edu.cn"));
        assert!(host_valid("sjtu-data"));
        assert_eq!(shell_quote("x'y"), "'x'\\''y'");
    }
}
