//! Non-secret connection identity retained when an operator SSH alias changes.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshEndpoint {
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub host_key_alias: String,
}
impl SshEndpoint {
    /// OpenSSH expands local configuration only; no network connection is made.
    pub fn capture(program: &str, alias: &str) -> Result<Self, String> {
        let output = std::process::Command::new(program)
            .args(["-G", "--", alias])
            .output()
            .map_err(|_| "cannot resolve SSH endpoint configuration")?;
        if !output.status.success() || output.stdout.len() > 1024 * 1024 {
            return Err("cannot resolve SSH endpoint configuration".into());
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| "invalid SSH endpoint configuration")?;
        let value = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{name} ")))
                .map(str::to_owned)
        };
        let hostname = value("hostname").ok_or("SSH hostname missing")?;
        let user = value("user").ok_or("SSH user missing")?;
        let port = value("port")
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|n| *n > 0)
            .ok_or("SSH port missing")?;
        let host_key_alias = value("hostkeyalias")
            .filter(|s| s != "none")
            .unwrap_or_else(|| hostname.clone());
        let result = Self {
            hostname,
            user,
            port,
            host_key_alias,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), String> {
        for value in [&self.hostname, &self.user, &self.host_key_alias] {
            if value.is_empty()
                || value.len() > 256
                || value.starts_with('-')
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-:@[]".contains(&b))
            {
                return Err("invalid SSH endpoint identity".into());
            }
        }
        if self.port == 0 {
            return Err("invalid SSH endpoint port".into());
        }
        Ok(())
    }
    pub fn options(&self) -> Vec<String> {
        vec![
            "-o".into(),
            format!("Hostname={}", self.hostname),
            "-o".into(),
            format!("User={}", self.user),
            "-o".into(),
            format!("Port={}", self.port),
            "-o".into(),
            format!("HostKeyAlias={}", self.host_key_alias),
        ]
    }
}
