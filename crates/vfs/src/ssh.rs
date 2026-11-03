//! Read-only, bounded-memory access to an operator-owned SSH data home.
//! OpenSSH owns authentication and host identity. No password/key is accepted
//! by this backend, and no dataset is mirrored onto the coordinator.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use opendal::raw::{Access, AccessorInfo, OpList, OpRead, OpStat, RpList, RpRead, RpStat, oio};
use opendal::{Buffer, Capability, EntryMode, Error, ErrorKind, Metadata, Operator, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SCRIPT: &str = include_str!("ssh_read.py");
const CHUNK: u64 = 1024 * 1024;
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

/// An authorized reference for same-host worker staging. It is metadata only;
/// the worker must still enforce its own resource, host and path admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshLocation {
    pub endpoint: Option<crate::SshEndpoint>,
    pub alias: String,
    /// Operator-owned backend root; remote consumers must confine opens here.
    pub root: String,
    pub path: String,
}

#[derive(Debug, Clone)]
struct SshAccess {
    endpoint: Option<crate::SshEndpoint>,
    alias: String,
    root: String,
    python: String,
    info: Arc<AccessorInfo>,
    #[cfg(test)]
    local_test_transport: bool,
}

fn failure(kind: ErrorKind, message: &'static str) -> Error {
    Error::new(kind, message)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
fn build(alias: &str, root: &str, python: &str) -> Result<Operator> {
    Ok(opendal::OperatorBuilder::new(build_access(alias, root, python)?).finish())
}

pub(crate) fn build_pinned(
    alias: &str,
    root: &str,
    python: &str,
    endpoint: Option<crate::SshEndpoint>,
) -> Result<Operator> {
    let mut access = build_access(alias, root, python)?;
    if let Some(endpoint) = &endpoint {
        endpoint
            .validate()
            .map_err(|_| failure(ErrorKind::ConfigInvalid, "invalid SSH endpoint identity"))?;
    }
    access.endpoint = endpoint;
    Ok(opendal::OperatorBuilder::new(access).finish())
}

fn build_access(alias: &str, root: &str, python: &str) -> Result<SshAccess> {
    let executable_component = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    };
    let valid_python = if let Some(absolute) = python.strip_prefix('/') {
        absolute.split('/').all(executable_component)
    } else {
        executable_component(python)
    };
    if !valid_python {
        return Err(failure(
            ErrorKind::ConfigInvalid,
            "SSH Python must be one executable name or canonical absolute path",
        ));
    }
    if alias.is_empty()
        || alias.starts_with('-')
        || !alias
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(failure(
            ErrorKind::ConfigInvalid,
            "SSH requires a configured OpenSSH host alias",
        ));
    }
    if !root.starts_with('/')
        || root == "/"
        || root.contains('\0')
        || root.split('/').any(|part| part == "." || part == "..")
    {
        return Err(failure(
            ErrorKind::ConfigInvalid,
            "SSH root must be a non-root canonical absolute directory",
        ));
    }
    let root = root.trim_end_matches('/').to_string();
    let info = AccessorInfo::default();
    info.set_scheme("ssh")
        .set_root(&root)
        .set_native_capability(Capability {
            stat: true,
            read: true,
            list: true,
            shared: true,
            ..Default::default()
        });
    Ok(SshAccess {
        endpoint: None,
        alias: alias.into(),
        root,
        python: python.into(),
        info: Arc::new(info),
        #[cfg(test)]
        local_test_transport: false,
    })
}

impl SshAccess {
    async fn request(&self, mut request: Value) -> Result<(Value, Vec<u8>)> {
        request["root"] = json!(self.root);
        let input = serde_json::to_vec(&request)
            .map_err(|_| failure(ErrorKind::Unexpected, "SSH request serialization failed"))?;
        let command = format!("{} -I -c {}", quote(&self.python), quote(SCRIPT));
        // The remote shell only sees fixed source. Paths and file identities
        // travel as JSON on stdin, so shell metacharacters are never expanded.
        let mut executable = tokio::process::Command::new("ssh");
        if let Some(endpoint) = &self.endpoint {
            executable.args(endpoint.options());
        }
        executable.args([
            "-T",
            "-oBatchMode=yes",
            "-oStrictHostKeyChecking=yes",
            "-oControlMaster=no",
            "-oControlPath=none",
            "-oControlPersist=no",
            "-oConnectTimeout=15",
            "-oServerAliveInterval=15",
            "-oServerAliveCountMax=2",
            "--",
            &self.alias,
            &command,
        ]);
        #[cfg(test)]
        if self.local_test_transport {
            executable = tokio::process::Command::new(&self.python);
            executable.args(["-I", "-c", SCRIPT]);
        }
        let mut child = executable
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| failure(ErrorKind::Unexpected, "SSH reader failed to start"))?;
        let operation = async {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH input unavailable"))?;
            stdin
                .write_all(&input)
                .await
                .map_err(|_| failure(ErrorKind::Unexpected, "SSH request failed"))?;
            drop(stdin);
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH response unavailable"))?;
            let mut output = Vec::new();
            stdout
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut output)
                .await
                .map_err(|_| failure(ErrorKind::Unexpected, "SSH response failed"))?;
            if output.len() as u64 > MAX_RESPONSE {
                return Err(failure(
                    ErrorKind::Unexpected,
                    "SSH response exceeded bounded limit",
                ));
            }
            let status = child
                .wait()
                .await
                .map_err(|_| failure(ErrorKind::Unexpected, "SSH status unavailable"))?;
            if !status.success() {
                return Err(failure(ErrorKind::Unexpected, "SSH remote reader failed"));
            }
            let split = output
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH response header missing"))?;
            let header: Value = serde_json::from_slice(&output[..split])
                .map_err(|_| failure(ErrorKind::Unexpected, "SSH response header invalid"))?;
            if header["ok"] != true {
                if header["error"] == "unsupported_runtime" {
                    return Err(failure(
                        ErrorKind::ConfigInvalid,
                        "SSH Python requires version 3.7+, confined fd operations and nonzero f_fsid",
                    ));
                }
                let kind = match header["error"].as_str() {
                    Some("not_found") => ErrorKind::NotFound,
                    Some("permission_denied") => ErrorKind::PermissionDenied,
                    Some("changed") => ErrorKind::ConditionNotMatch,
                    _ => ErrorKind::Unexpected,
                };
                return Err(failure(kind, "SSH read-only operation refused"));
            }
            let body = output.split_off(split + 1);
            Ok((header, body))
        };
        tokio::time::timeout(Duration::from_secs(60), operation)
            .await
            .map_err(|_| failure(ErrorKind::Unexpected, "SSH read-only operation timed out"))?
    }
}

fn metadata(value: &Value) -> Result<Metadata> {
    let directory = value["directory"]
        .as_bool()
        .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH file mode missing"))?;
    let size = value["size"]
        .as_u64()
        .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH file size missing"))?;
    let mut metadata = Metadata::new(if directory {
        EntryMode::DIR
    } else {
        EntryMode::FILE
    })
    .with_content_length(size);
    if let Some(modified) = value["modified"].as_i64()
        && let Ok(time) = opendal::raw::Timestamp::from_millisecond(modified)
    {
        metadata.set_last_modified(time);
    }
    Ok(metadata)
}

impl Access for SshAccess {
    type Reader = SshReader;
    type Writer = ();
    type Lister = SshLister;
    type Deleter = ();
    type Copier = ();

    fn info(&self) -> Arc<AccessorInfo> {
        Arc::clone(&self.info)
    }

    async fn stat(&self, path: &str, _: OpStat) -> Result<RpStat> {
        let (header, _) = self.request(json!({"op":"stat", "path":path})).await?;
        Ok(RpStat::new(metadata(&header)?))
    }

    async fn read(&self, path: &str, args: OpRead) -> Result<(RpRead, SshReader)> {
        let (header, _) = self.request(json!({"op":"stat", "path":path})).await?;
        let info = metadata(&header)?;
        if info.mode().is_dir() {
            return Err(failure(
                ErrorKind::IsADirectory,
                "SSH read requires a regular file",
            ));
        }
        let offset = args.range().offset();
        let available = info.content_length().saturating_sub(offset);
        let remaining = args.range().size().unwrap_or(available).min(available);
        Ok((
            RpRead::default(),
            SshReader {
                access: self.clone(),
                path: path.into(),
                identity: header["identity"].clone(),
                offset,
                remaining,
            },
        ))
    }

    async fn list(&self, path: &str, _: OpList) -> Result<(RpList, SshLister)> {
        let (header, _) = self.request(json!({"op":"list", "path":path})).await?;
        let entries = header["entries"]
            .as_array()
            .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH directory entries missing"))?;
        let mut result = VecDeque::new();
        for entry in entries {
            let name = entry["name"]
                .as_str()
                .ok_or_else(|| failure(ErrorKind::Unexpected, "SSH entry name missing"))?;
            if name.contains('/') || name == "." || name == ".." || name.contains('\0') {
                return Err(failure(ErrorKind::Unexpected, "SSH entry name invalid"));
            }
            let metadata = metadata(entry)?;
            let mut key = format!(
                "{}{name}",
                if path.is_empty() {
                    String::new()
                } else {
                    format!("{}/", path.trim_end_matches('/'))
                }
            );
            if metadata.mode().is_dir() {
                key.push('/');
            }
            result.push_back(oio::Entry::new(&key, metadata));
        }
        Ok((RpList::default(), SshLister(result)))
    }
}

struct SshReader {
    access: SshAccess,
    path: String,
    identity: Value,
    offset: u64,
    remaining: u64,
}

impl oio::Read for SshReader {
    async fn read(&mut self) -> Result<Buffer> {
        if self.remaining == 0 {
            return Ok(Buffer::new());
        }
        let size = self.remaining.min(CHUNK);
        let (header, body) = self
            .access
            .request(json!({"op":"read", "path":self.path,
            "identity":self.identity, "offset":self.offset, "size":size}))
            .await?;
        if body.len() as u64 != size
            || header["length"].as_u64() != Some(size)
            || header["sha256"].as_str() != Some(format!("{:x}", Sha256::digest(&body)).as_str())
            || header["identity"] != self.identity
        {
            return Err(failure(
                ErrorKind::ConditionNotMatch,
                "SSH content identity or length changed",
            ));
        }
        self.offset += size;
        self.remaining -= size;
        Ok(Buffer::from(body))
    }
}

struct SshLister(VecDeque<oio::Entry>);
impl oio::List for SshLister {
    async fn next(&mut self) -> Result<Option<oio::Entry>> {
        Ok(self.0.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MountedObjectStore, OpendalFileStorage, VfsManifest};

    fn test_python() -> &'static str {
        static PYTHON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        PYTHON.get_or_init(|| {
            // Test the same reader capability contract as production. A site
            // default may be Python 3.6 while a supported interpreter is also
            // installed; none of these candidates is a production fallback.
            let candidates = match std::env::var("AUTONOMICS_VFS_TEST_PYTHON") {
                Ok(explicit) => vec![explicit],
                Err(std::env::VarError::NotPresent) => [
                    "python3", "/usr/bin/python3.11", "python3.11", "python3.9",
                ].into_iter().map(str::to_owned).collect(),
                Err(error) => panic!("invalid AUTONOMICS_VFS_TEST_PYTHON: {error}"),
            };
            for candidate in &candidates {
                let status = std::process::Command::new(candidate)
                    .args(["-I", "-c", "import sys; scope={'__name__':'ssh_runtime_probe'}; exec(compile(sys.argv[1], '<ssh-reader>', 'exec'), scope); scope['validate_runtime']()", SCRIPT])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                if status.is_ok_and(|status| status.success()) {
                    return candidate.clone();
                }
            }
            panic!("SSH protocol tests require Python 3.7+ with confined fd operations; checked {candidates:?}; set AUTONOMICS_VFS_TEST_PYTHON explicitly");
        })
    }

    fn local_operator(root: &std::path::Path) -> Operator {
        let root = root.canonicalize().unwrap();
        let mut access = build_access("toy-host", root.to_str().unwrap(), test_python()).unwrap();
        access.local_test_transport = true;
        opendal::OperatorBuilder::new(access).finish()
    }

    #[tokio::test]
    async fn python_protocol_roundtrips_stat_ranges_and_directory_listing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(
            root.path().join("nested/input '$(literal).bin"),
            b"abcdefghij",
        )
        .unwrap();
        let operator = local_operator(root.path());
        let info = operator.stat("nested/input '$(literal).bin").await.unwrap();
        assert!(info.mode().is_file());
        assert_eq!(info.content_length(), 10);
        assert!(info.last_modified().is_some());
        assert!(operator.stat("nested/").await.unwrap().mode().is_dir());
        let bytes = operator
            .read_with("nested/input '$(literal).bin")
            .range(2..6)
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"cdef");
        assert_eq!(
            operator
                .read("nested/input '$(literal).bin")
                .await
                .unwrap()
                .to_vec(),
            b"abcdefghij"
        );
        let listing = operator.list("nested/").await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].path(), "nested/input '$(literal).bin");
        assert_eq!(listing[0].metadata().content_length(), 10);
        assert_eq!(
            operator.stat("missing").await.unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert!(operator.write("new", b"refused".to_vec()).await.is_err());
    }

    #[tokio::test]
    async fn open_reader_rejects_modified_input_and_symlink_targets() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input");
        std::fs::write(&input, vec![b'a'; CHUNK as usize + 2]).unwrap();
        let mut access = build_access(
            "toy-host",
            root.path().canonicalize().unwrap().to_str().unwrap(),
            test_python(),
        )
        .unwrap();
        access.local_test_transport = true;
        let (_, mut reader) = access.read("input", OpRead::default()).await.unwrap();
        assert_eq!(
            oio::Read::read(&mut reader).await.unwrap().len(),
            CHUNK as usize
        );
        std::fs::write(&input, b"changed").unwrap();
        assert_eq!(
            oio::Read::read(&mut reader).await.unwrap_err().kind(),
            ErrorKind::ConditionNotMatch
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&input, root.path().join("link")).unwrap();
            let operator = local_operator(root.path());
            assert_eq!(
                operator.stat("link").await.unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
            assert_eq!(operator.list("").await.unwrap().len(), 1);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_runtime_refusal_is_not_ignored_by_transport() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("synthetic-python");
        // A configured interpreter refusing its runtime contract must reach
        // the caller; silently using python3 instead would stat this root.
        std::fs::write(
            &executable,
            b"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"ok\":false,\"error\":\"unsupported_runtime\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut access = build_access(
            "toy-host",
            root.path().canonicalize().unwrap().to_str().unwrap(),
            executable.to_str().unwrap(),
        )
        .unwrap();
        access.local_test_transport = true;
        let operator = opendal::OperatorBuilder::new(access).finish();
        // OpenDAL synthesizes metadata for its empty root without calling
        // the accessor. A named path must reach the configured interpreter.
        let error = operator.stat("runtime-probe").await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
        assert!(error.to_string().contains("confined fd operations"));
    }

    #[test]
    fn rejects_shell_options_and_unbounded_roots() {
        for alias in ["", "-Fsecret", "sjtu;echo", "name@host", "bad host"] {
            assert!(build(alias, "/data/study", "python3").is_err());
        }
        for root in ["/", "relative", "/study/../other", "/study/./raw"] {
            assert!(build("sjtu", root, "python3").is_err());
        }
        let operator = build("sjtu", "/data/study", "python3").unwrap();
        assert!(operator.info().full_capability().read);
        assert!(!operator.info().full_capability().write);
        for python in [
            "",
            "-I",
            "python3 -I",
            "../python3",
            "bin/python3",
            "/usr/../bin/python3",
            "/usr//bin/python3",
            "python3;echo",
            "/",
        ] {
            assert!(build("sjtu", "/data/study", python).is_err());
        }
        assert!(build("sjtu", "/data/study", "/usr/bin/python3.11").is_ok());
        assert!(build("sjtu", "/data/study", "python3.11").is_ok());
    }

    #[test]
    fn remote_references_obey_principal_and_mount_boundaries() {
        let manifest = VfsManifest::from_toml(
            r#"
[[backend]]
id = "cluster"
type = "ssh"
alias = "sjtu"
root = "/data/operator"
[[mount]]
path = "/research"
backend = "cluster"
source = "bmc"
read_only = true
[mount.permissions]
owner = { uid = 10, gid = 10 }
file_mode = "0600"
directory_mode = "0700"
"#,
        )
        .unwrap();
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let root = tempfile::tempdir().unwrap();
        let storage = OpendalFileStorage::with_mounts(root.path(), mounts);
        let location = storage
            .ssh_location("vfs:///research/raw/file.bin")
            .unwrap()
            .unwrap();
        assert_eq!(
            location,
            SshLocation {
                endpoint: None,
                alias: "sjtu".into(),
                root: "/data/operator".into(),
                path: "/data/operator/bmc/raw/file.bin".into(),
            }
        );
        assert!(
            storage
                .ssh_location("/scratch/local.bin")
                .unwrap()
                .is_none()
        );
        assert!(storage.check_writable("/research/new.bin").is_err());
        let restricted = storage.with_principal(crate::permission::VfsPrincipal::new(999, 999));
        assert_eq!(
            restricted
                .ssh_location("/research/raw/file.bin")
                .unwrap_err()
                .kind(),
            ErrorKind::PermissionDenied
        );
        let mut writable = manifest;
        writable.mount[0].read_only = false;
        assert!(MountedObjectStore::from_manifest(&writable).is_err());
        writable.mount[0].read_only = true;
        writable.mount[0].source = "/outside".into();
        assert!(MountedObjectStore::from_manifest(&writable).is_err());
    }
}
