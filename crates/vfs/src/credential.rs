//! SSH credential vault: sealed storage for passwords and TOTP secrets,
//! with prompt classification for askpass injection.
//!
//! Clusters that forbid key auth and require one-time passwords cannot be
//! served by the stateless BatchMode contract alone. The vault keeps one
//! sealed file per daemon (`0600`, Argon2id-derived key, AES-256-GCM
//! authenticated encryption); the daemon unlocks it once at startup and
//! serves answers to the `autonomics-askpass` helper over a same-user
//! unix socket. Secrets exist in the clear only in the daemon's memory
//! and per answer in the ssh process that asked.
//!
//! Threat model ("relatively secure"): the sealed file protects secrets
//! at rest against disk theft and backup leakage; the socket restricts
//! retrieval to the same uid. Nothing defends against a compromised
//! daemon or a hostile same-uid process — that is out of scope for any
//! in-process credential store.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use totp_rs::{Algorithm, Secret, TOTP};
use zeroize::Zeroizing;

const MAGIC: &[u8; 10] = b"AUTOVAULT1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

fn sealed_error(reason: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, reason)
}

/// One host's credential material. Every field is optional: an entry may
/// hold only a password, only an OTP secret, or both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Base32-encoded TOTP shared secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp_secret: Option<String>,
    #[serde(default = "default_totp_digits")]
    pub totp_digits: u32,
    #[serde(default = "default_totp_period")]
    pub totp_period: u64,
    /// Case-insensitive substrings identifying the OTP prompt. Defaults
    /// cover the common HPC setups.
    #[serde(default = "default_otp_prompt_tokens")]
    pub otp_prompt_tokens: Vec<String>,
}

fn default_totp_digits() -> u32 {
    6
}
fn default_totp_period() -> u64 {
    30
}

impl Default for SshCredential {
    fn default() -> Self {
        Self {
            password: None,
            totp_secret: None,
            totp_digits: default_totp_digits(),
            totp_period: default_totp_period(),
            otp_prompt_tokens: default_otp_prompt_tokens(),
        }
    }
}
fn default_otp_prompt_tokens() -> Vec<String> {
    ["otp", "verification", "passcode", "one-time", "token"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The unlocked vault: credential entries keyed by SSH host alias (the
/// `"*"` key matches any host as a fallback).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialVault {
    #[serde(default)]
    pub entries: BTreeMap<String, SshCredential>,
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let params = argon2::Params::new(64 * 1024, 3, 1, Some(KEY_LEN))
        .expect("hardcoded Argon2 parameters are valid");
    let argon = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut_slice())
        .expect("hardcoded Argon2 parameters are valid");
    key
}

impl CredentialVault {
    /// Seal the vault into the on-disk format:
    /// `AUTOVAULT1 || salt(16) || nonce(12) || AES-256-GCM(JSON)`.
    pub fn seal(&self, passphrase: &str) -> std::io::Result<Vec<u8>> {
        let mut salt = [0u8; SALT_LEN];
        let mut nonce_bytes = [0u8; NONCE_LEN];
        // /dev/urandom via getrandom is the auditable choice here; fall
        // back is not acceptable for a credential store.
        getrandom_fill(&mut salt)?;
        getrandom_fill(&mut nonce_bytes)?;
        let key_bytes = derive_key(passphrase, &salt);
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes.as_ref()[..KEY_LEN]);
        let cipher = Aes256Gcm::new(key);
        let plaintext = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        let sealed = cipher
            .encrypt(
                &Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &plaintext,
                    aad: MAGIC,
                },
            )
            .map_err(|_| sealed_error("vault encryption failed"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Open a sealed vault. Fails closed on any tampering (GCM tag),
    /// wrong passphrase, or truncated header.
    pub fn open(bytes: &[u8], passphrase: &str) -> std::io::Result<Self> {
        let header = MAGIC.len() + SALT_LEN + NONCE_LEN;
        if bytes.len() <= header || &bytes[..MAGIC.len()] != MAGIC {
            return Err(sealed_error("not an autonomics credential vault"));
        }
        let salt = &bytes[MAGIC.len()..MAGIC.len() + SALT_LEN];
        let nonce_bytes = &bytes[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
        let key_bytes = derive_key(passphrase, salt);
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes.as_ref()[..KEY_LEN]);
        let cipher = Aes256Gcm::new(key);
        let plain = cipher
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload {
                    msg: &bytes[header..],
                    aad: MAGIC,
                },
            )
            .map_err(|_| sealed_error("wrong passphrase or corrupted vault"))?;
        serde_json::from_slice(&plain).map_err(std::io::Error::other)
    }

    fn entry_for(&self, alias: &str) -> Option<&SshCredential> {
        self.entries
            .get(alias)
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(alias))
                    .map(|(_, value)| value)
            })
            .or_else(|| self.entries.get("*"))
    }

    /// Answer one askpass prompt for one host. `None` means "the vault
    /// cannot answer this prompt" — ssh then fails fast (BatchMode
    /// semantics are preserved for everything the vault does not cover).
    pub fn answer(&self, alias: &str, prompt: &str, now: SystemTime) -> Option<String> {
        let entry = self.entry_for(alias)?;
        let lowered = prompt.to_ascii_lowercase();
        let otp_prompt = entry
            .otp_prompt_tokens
            .iter()
            .any(|token| lowered.contains(&token.to_ascii_lowercase()));
        if otp_prompt {
            let secret = entry.totp_secret.as_deref()?;
            return totp_code(secret, entry.totp_digits, entry.totp_period, now).ok();
        }
        if lowered.contains("password") || lowered.contains("passphrase") {
            return entry.password.clone();
        }
        None
    }
}

fn getrandom_fill(buffer: &mut [u8]) -> std::io::Result<()> {
    // getrandom(2) directly: no dependency on a crate at this time.
    #[cfg(unix)]
    {
        let mut filled = 0;
        while filled < buffer.len() {
            let count = unsafe {
                libc::getrandom(
                    buffer.as_mut_ptr().add(filled) as *mut core::ffi::c_void,
                    buffer.len() - filled,
                    0,
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error());
            }
            filled += count as usize;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "credential vault requires unix getrandom",
        ))
    }
}

/// Current TOTP code via [`totp_rs`]: RFC 6238 over HMAC-SHA1, the
/// profile essentially every HPC OTP deployment uses. Skew of 1 step
/// covers small coordinator/cluster clock drift.
pub fn totp_code(
    secret_base32: &str,
    digits: u32,
    period: u64,
    now: SystemTime,
) -> Result<String, &'static str> {
    let bytes = Secret::Encoded(secret_base32.to_string())
        .to_bytes()
        .map_err(|_| "invalid base32 TOTP secret")?;
    let totp = TOTP::new(
        Algorithm::SHA1,
        digits.clamp(1, 10) as usize,
        1,
        period.max(1),
        bytes,
    )
    .map_err(|_| "invalid TOTP parameters")?;
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock before the unix epoch")?
        .as_secs();
    Ok(totp.generate(seconds))
}

/// The one-line request `autonomics-askpass` sends to the vault agent.
#[derive(Serialize, Deserialize)]
pub struct VaultRequest {
    pub alias: String,
    pub prompt: String,
}

/// The agent's one-line reply.
#[derive(Serialize, Deserialize)]
pub struct VaultReply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// Serve the unlocked vault over a same-user unix socket. One request
/// per connection, one line in, one line out. Runs until the listener
/// fails or the task is dropped; abort it to stop.
pub async fn serve_vault(
    vault: CredentialVault,
    socket_path: std::path::PathBuf,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    }
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => return Err(error),
        };
        let vault = vault.clone();
        tokio::spawn(async move {
            let _ = answer_one(stream, vault).await;
        });
    }

    async fn answer_one(stream: UnixStream, vault: CredentialVault) -> std::io::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut line = String::new();
        let mut reader = BufReader::new(reader);
        reader.read_line(&mut line).await?;
        let request: VaultRequest =
            serde_json::from_str(line.trim_end()).map_err(std::io::Error::other)?;
        let reply = VaultReply {
            answer: vault.answer(&request.alias, &request.prompt, SystemTime::now()),
        };
        writer
            .write_all(serde_json::to_string(&reply)?.as_bytes())
            .await?;
        writer.write_all(b"\n").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc_vector_secret() -> String {
        // RFC 6238 test secret ("12345678901234567890"), base32 encoded.
        "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into()
    }

    #[test]
    fn totp_matches_rfc6238_vectors() {
        let secret = rfc_vector_secret();
        // Time 59 → counter 1 → 94287082 (8 digits, SHA1).
        let t = UNIX_EPOCH + std::time::Duration::from_secs(59);
        assert_eq!(totp_code(&secret, 8, 30, t).unwrap(), "94287082");
        // Time 1111111109 → 07081804.
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_111_111_109);
        assert_eq!(totp_code(&secret, 8, 30, t).unwrap(), "07081804");
    }

    #[test]
    fn vault_seal_open_roundtrip_rejects_wrong_passphrase() {
        let mut vault = CredentialVault::default();
        vault.entries.insert(
            "hpc-otp".into(),
            SshCredential {
                password: Some("hunter2".into()),
                totp_secret: Some(rfc_vector_secret()),
                ..Default::default()
            },
        );
        let sealed = vault.seal("correct horse").unwrap();
        let reopened = CredentialVault::open(&sealed, "correct horse").unwrap();
        assert!(
            reopened
                .answer("hpc-otp", "Password:", SystemTime::now())
                .as_deref()
                == Some("hunter2")
        );
        assert!(
            CredentialVault::open(&sealed, "wrong").is_err(),
            "wrong passphrase must fail closed"
        );
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(CredentialVault::open(&tampered, "correct horse").is_err());
    }

    #[test]
    fn prompts_route_between_password_and_totp() {
        let mut vault = CredentialVault::default();
        vault.entries.insert(
            "hpc".into(),
            SshCredential {
                password: Some("pw".into()),
                totp_secret: Some(rfc_vector_secret()),
                totp_digits: 8,
                ..Default::default()
            },
        );
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_111_111_109);
        assert_eq!(vault.answer("hpc", "Password:", t).as_deref(), Some("pw"));
        assert_eq!(
            vault.answer("hpc", "Enter OTP code:", t).as_deref(),
            Some("07081804")
        );
        assert_eq!(
            vault.answer("hpc", "Verification code:", t).as_deref(),
            Some("07081804")
        );
        assert_eq!(vault.answer("hpc", "Are you sure?", t), None);
        // Fallback entry and case-insensitive alias match.
        vault.entries.insert(
            "*".into(),
            SshCredential {
                password: Some("default-pw".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            vault.answer("HPC", "password please", t).as_deref(),
            Some("pw")
        );
        assert_eq!(
            vault.answer("other-host", "Password:", t).as_deref(),
            Some("default-pw")
        );
    }

    #[tokio::test]
    async fn vault_agent_answers_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("vault.sock");
        let mut vault = CredentialVault::default();
        vault.entries.insert(
            "hpc".into(),
            SshCredential {
                password: Some("socket-pw".into()),
                ..Default::default()
            },
        );
        let serving = tokio::spawn({
            let socket = socket.clone();
            let vault = vault.clone();
            async move { serve_vault(vault, socket).await }
        });
        let reply: VaultReply = ask_socket(&socket, "hpc", "Password:").await;
        assert_eq!(reply.answer.as_deref(), Some("socket-pw"));
        serving.abort();
    }

    async fn ask_socket(socket: &std::path::Path, alias: &str, prompt: &str) -> VaultReply {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        // The serving task binds asynchronously; poll briefly for it.
        let mut stream = None;
        for _ in 0..100 {
            match tokio::net::UnixStream::connect(socket).await {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        let stream = stream.expect("vault agent socket never appeared");
        let (reader, mut writer) = stream.into_split();
        let request = serde_json::to_string(&VaultRequest {
            alias: alias.into(),
            prompt: prompt.into(),
        })
        .unwrap();
        writer.write_all(request.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(reader);
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim_end()).unwrap()
    }
}
