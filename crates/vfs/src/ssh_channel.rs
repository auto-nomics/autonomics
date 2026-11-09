//! Process-wide SSH channel multiplexing.
//!
//! Every SSH transport in this system (execution, file access, scheduler
//! tools) shells out to OpenSSH per operation, keeping zero connection
//! state in the process. Configuring multiplexing lets consecutive
//! invocations reuse one authenticated master connection per destination
//! endpoint: each invocation remains an independent exec session —
//! auditing, timeouts and failure semantics are unchanged — but the
//! TCP/KEX/auth handshake is paid once per idle window instead of once
//! per operation.
//!
//! `ControlMaster=auto` masters self-heal: a dead master makes the next
//! invocation start a fresh one, and existing uncertainty protocols
//! (submission intents, adoption, stop proofs) already tolerate a
//! connection failing at any point. The ControlPath keys on `%r@%h:%p`,
//! so endpoint-pinned options (`-o Hostname=…`) keep distinct endpoints
//! on distinct masters and alias-drift protection is unaffected.
//!
//! Disabled by default; the daemon configures it once at startup.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone)]
struct Configured {
    control_path: String,
    persist: String,
}

static MULTIPLEX: RwLock<Option<Configured>> = RwLock::new(None);

/// Enable connection multiplexing for every SSH channel in this process.
///
/// `control_dir` must be absolute; it is created with 0700 permissions if
/// missing. `persist` is an OpenSSH duration (for example `10m` or `yes`)
/// bounding how long an idle master survives.
pub fn configure(control_dir: &Path, persist: &str) -> std::io::Result<()> {
    if !control_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ssh control directory must be absolute",
        ));
    }
    if persist.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ssh ControlPersist must be a non-empty OpenSSH duration",
        ));
    }
    std::fs::create_dir_all(control_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(control_dir)?;
        std::fs::set_permissions(
            control_dir,
            std::fs::Permissions::from_mode(metadata.permissions().mode() & !0o077 | 0o700),
        )?;
    }
    let configured = Configured {
        control_path: format!("{}/cm-%r@%h:%p", control_dir.display()),
        persist: persist.trim().to_string(),
    };
    *MULTIPLEX.write().expect("ssh channel lock poisoned") = Some(configured);
    Ok(())
}

/// The `Control*` channel options every SSH invocation should carry,
/// without the `-o` prefix. When multiplexing is unconfigured (the
/// default) these restate the stateless contract explicitly.
pub fn options() -> Vec<String> {
    match MULTIPLEX
        .read()
        .expect("ssh channel lock poisoned")
        .as_ref()
    {
        Some(configured) => vec![
            "ControlMaster=auto".into(),
            format!("ControlPath={}", configured.control_path),
            format!("ControlPersist={}", configured.persist),
        ],
        None => vec![
            "ControlMaster=no".into(),
            "ControlPath=none".into(),
            "ControlPersist=no".into(),
        ],
    }
}

/// Whether multiplexing is currently active.
pub fn is_configured() -> bool {
    MULTIPLEX
        .read()
        .expect("ssh channel lock poisoned")
        .is_some()
}

/// Clear the configuration. Test-only: production configures once.
#[cfg(test)]
pub(crate) fn reset() {
    *MULTIPLEX.write().expect("ssh channel lock poisoned") = None;
}

/// Serializes the tests that mutate process-global channel state.
#[cfg(test)]
static TEST_ORDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_channels_stay_stateless() {
        let _order = TEST_ORDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset();
        assert_eq!(
            options(),
            vec![
                "ControlMaster=no".to_string(),
                "ControlPath=none".to_string(),
                "ControlPersist=no".to_string(),
            ]
        );
        assert!(!is_configured());
    }

    #[test]
    fn configured_channels_reuse_masters_per_endpoint() {
        let _order = TEST_ORDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset();
        let dir = tempfile::tempdir().unwrap();
        configure(dir.path(), "10m").unwrap();
        assert!(is_configured());
        let options = options();
        assert_eq!(options[0], "ControlMaster=auto");
        assert!(
            options[1].contains("cm-%r@%h:%p"),
            "ControlPath must key on endpoint identity: {}",
            options[1]
        );
        assert_eq!(options[2], "ControlPersist=10m");
        assert!(
            options[1].starts_with("ControlPath=/"),
            "control path must be rooted at an absolute directory: {}",
            options[1]
        );
        reset();
    }

    #[test]
    fn configuration_rejects_relative_directories() {
        let _order = TEST_ORDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset();
        assert!(configure(Path::new("relative/cm"), "10m").is_err());
        assert!(!is_configured());
        reset();
    }
}

// ── credential injection ────────────────────────────────────────────────

/// The vault agent socket, when the daemon unlocked a credential vault.
static VAULT_SOCKET: RwLock<Option<String>> = RwLock::new(None);

/// The env var carrying the agent socket to spawned ssh processes.
pub const VAULT_SOCK_ENV: &str = "AUTONOMICS_VAULT_SOCK";
/// The env var tagging the host alias for the askpass helper.
pub const VAULT_ALIAS_ENV: &str = "AUTONOMICS_VAULT_ALIAS";

/// Password prompts are allowed only when a vault-backed responder is available.
pub fn batch_mode_option() -> &'static str {
    if VAULT_SOCKET.read().expect("ssh channel lock poisoned").is_some()
        && askpass_executable().is_some()
    {
        "BatchMode=no"
    } else {
        "BatchMode=yes"
    }
}

/// Advertise the vault agent socket so every spawned SSH channel can
/// answer password and OTP prompts through `autonomics-askpass`.
pub fn configure_vault_agent(socket_path: &Path) {
    *VAULT_SOCKET.write().expect("ssh channel lock poisoned") =
        Some(socket_path.display().to_string());
}

/// The SSH_ASKPASS executable. Resolution order:
///
/// 1. `AUTONOMICS_ASKPASS_BIN` — explicit override (a binary or script
///    answering prompts on stdout);
/// 2. the unified `autonomics` CLI sitting next to the current
///    executable, wrapped once in a tiny `exec … askpass "$@"` script —
///    deployments ship exactly one binary and every command is its
///    subcommand;
/// 3. a sibling `autonomics-askpass` binary (development layouts).
///
/// The generated wrapper is cached per process.
fn askpass_executable() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("AUTONOMICS_ASKPASS_BIN") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let current = std::env::current_exe().ok()?;
    let directory = current.parent()?;
    let unified = directory.join("autonomics");
    if unified.is_file() {
        return askpass_wrapper_for(&unified);
    }
    let sibling = directory.join("autonomics-askpass");
    sibling.is_file().then_some(sibling)
}

fn askpass_wrapper_for(unified: &Path) -> Option<PathBuf> {
    static WRAPPER: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    WRAPPER
        .get_or_init(|| {
            // SAFETY: geteuid reads the current credential.
            let uid = unsafe { libc::geteuid() };
            let directory = std::env::temp_dir().join(format!("autonomics-askpass-{uid}"));
            std::fs::create_dir_all(&directory).ok()?;
            let quoted = unified.display().to_string().replace('\'', "'\\''");
            let wrapper = directory.join("askpass.sh");
            std::fs::write(
                &wrapper,
                format!("#!/bin/sh\nexec '{quoted}' askpass \"$@\"\n"),
            )
            .ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).ok()?;
            }
            Some(wrapper)
        })
        .clone()
}

/// Attach vault-backed credential answering to one ssh invocation. When
/// no vault agent is configured this is a no-op and BatchMode semantics
/// are untouched. `SSH_ASKPASS_REQUIRE=force` makes OpenSSH consult the
/// helper even with a terminal attached.
pub fn attach_credentials(command: &mut impl SshCommandEnv, alias: &str) {
    let socket = VAULT_SOCKET
        .read()
        .expect("ssh channel lock poisoned")
        .clone();
    let Some(socket) = socket else {
        return;
    };
    let Some(askpass) = askpass_executable() else {
        return;
    };
    command.set_env("SSH_ASKPASS", &askpass.display().to_string());
    command.set_env("SSH_ASKPASS_REQUIRE", "force");
    command.set_env(VAULT_SOCK_ENV, &socket);
    command.set_env(VAULT_ALIAS_ENV, alias);
}

/// The env-setting surface shared by sync and async process commands.
pub trait SshCommandEnv {
    fn set_env(&mut self, key: &str, value: &str);
}

impl SshCommandEnv for std::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

impl SshCommandEnv for tokio::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

#[cfg(test)]
mod credential_injection_tests {
    use super::*;

    #[test]
    fn injection_is_absent_until_configured_and_then_tags_the_alias() {
        let _order = TEST_ORDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut command = std::process::Command::new("ssh");
        attach_credentials(&mut command, "hpc");
        assert!(
            !command
                .get_envs()
                .any(|(key, _)| key == std::ffi::OsStr::new("SSH_ASKPASS"))
        );

        let dir = tempfile::tempdir().unwrap();
        let fake_askpass = dir.path().join("autonomics-askpass");
        std::fs::write(&fake_askpass, b"#!/bin/sh\n").unwrap();
        // askpass_executable honors the explicit override, so point it at
        // the fake without needing a sibling of the test binary.
        // Single test mutates the process env; the injection tests are
        // serialized by construction (this is the only one).
        unsafe { std::env::set_var("AUTONOMICS_ASKPASS_BIN", &fake_askpass) };
        configure_vault_agent(&dir.path().join("vault.sock"));
        let mut command = std::process::Command::new("ssh");
        attach_credentials(&mut command, "hpc-otp");
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new(VAULT_ALIAS_ENV)).unwrap(),
            &Some(std::ffi::OsStr::new("hpc-otp"))
        );
        assert!(
            env.get(std::ffi::OsStr::new("SSH_ASKPASS_REQUIRE"))
                .is_some()
        );
        assert!(env.get(std::ffi::OsStr::new(VAULT_SOCK_ENV)).is_some());
        // Reset the process-global state so other tests stay clean.
        *VAULT_SOCKET.write().unwrap() = None;
        unsafe { std::env::remove_var("AUTONOMICS_ASKPASS_BIN") };
    }
}
