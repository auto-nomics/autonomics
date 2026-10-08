//! Image build and push capabilities for environment development.
//!
//! [`ImageBuildConnection`] is deliberately separate from
//! [`crate::PodmanConnection`]: the run-path trait has many fake
//! implementations in node-bundle tests and must not be forced to carry
//! build/push surface it cannot honor.
//!
//! Every Podman invocation detaches stdin (`Stdio::null()`), matching the run
//! path: an attached Podman process consumes inherited stdin and steals
//! terminal input from whatever host process shares the tty.
//!
//! Push reads the digest from `--digestfile`, never from local inspect. The
//! local containers-storage digest can differ from the pushed manifest digest
//! (push may recompress layers), so only the digest file describes what the
//! registry now serves. The same rule applies to builds that will later be
//! pushed: `build` reports the local digest for local activation, and the
//! publisher re-pins the catalog with the digest returned by `push`.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use async_trait::async_trait;
use tokio::process::Command;

use crate::error::ContainerRuntimeError;
use crate::podman::{PodmanRuntime, async_podman_command};

/// Budget for the `podman inspect` that resolves a freshly built image.
const INSPECT_TIMEOUT_SECS: u64 = 60;
/// Default budget for one `podman push` of a multi-layer environment image.
pub const DEFAULT_PUSH_TIMEOUT_SECS: u64 = 3600;

/// One daemon-owned image build over a development workspace context.
#[derive(Debug, Clone)]
pub struct ImageBuildRequest {
    /// Daemon-owned absolute path of the build context (the environment
    /// workspace). Its untracked scratch state is acceptable: builds are
    /// infrastructure behavior, not agent-visible shell access.
    pub context_dir: PathBuf,
    /// Context-relative path of the Containerfile, e.g. `Containerfile`.
    pub containerfile: String,
    /// Local tag applied to the build, e.g.
    /// `localhost/auto-nomics/environments/bioconductor-extra:rsi-3`.
    pub tag: String,
    /// Build arguments in declaration order.
    pub build_args: Vec<(String, String)>,
    pub timeout_secs: u64,
}

/// The content-addressed identity of one built image.
///
/// `digest` is the local manifest digest; it pins local activation but is not
/// authoritative for a registry after a push (see [`ImagePushResult`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBuildResult {
    pub image_id: String,
    pub digest: String,
}

/// One daemon-owned push of a local image to the configured registry.
#[derive(Debug, Clone)]
pub struct ImagePushRequest {
    /// Local digest-pinned or tagged reference being pushed.
    pub local_reference: String,
    /// Tagged remote destination, e.g. `ghcr.io/auto-nomics/environments/x:abc123`.
    pub remote_reference: String,
    pub timeout_secs: u64,
}

/// The registry-side identity of one pushed image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePushResult {
    pub remote_reference: String,
    /// Manifest digest the registry now serves (from `--digestfile`).
    pub digest: String,
}

/// Build and push capability contract for environment images.
///
/// Production uses [`PodmanRuntime`]; hosts and tests inject fakes so agents
/// never gain raw Podman access.
#[async_trait]
pub trait ImageBuildConnection: Send + Sync {
    async fn build(
        &self,
        request: ImageBuildRequest,
    ) -> Result<ImageBuildResult, ContainerRuntimeError>;

    async fn push(
        &self,
        request: ImagePushRequest,
    ) -> Result<ImagePushResult, ContainerRuntimeError>;
}

#[async_trait]
impl ImageBuildConnection for PodmanRuntime {
    async fn build(
        &self,
        request: ImageBuildRequest,
    ) -> Result<ImageBuildResult, ContainerRuntimeError> {
        let args = build_image_args(&request)?;
        let program = self.config().program.clone();
        let built = tokio::time::timeout(
            Duration::from_secs(request.timeout_secs),
            async_podman_command(&program)
                .arg("build")
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        let output = command_output("build", built)?;
        if !output.status.success() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`{} build` failed with status {}: {}",
                program,
                output.status.code().unwrap_or(1),
                crate::connection::truncate_captured_bytes(&output.stderr)
            )));
        }

        let inspected = tokio::time::timeout(
            Duration::from_secs(INSPECT_TIMEOUT_SECS),
            async_podman_command(&program)
                .args(["inspect", "--format", "{{.Id}}\n{{.Digest}}"])
                .arg(&request.tag)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        let output = command_output("inspect", inspected)?;
        if !output.status.success() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`{} inspect {}` failed with status {}: {}",
                program,
                request.tag,
                output.status.code().unwrap_or(1),
                crate::connection::truncate_captured_bytes(&output.stderr)
            )));
        }
        let (image_id, digest) = parse_inspect_id_digest(&String::from_utf8_lossy(&output.stdout))
            .map_err(ContainerRuntimeError::Invalid)?;
        Ok(ImageBuildResult { image_id, digest })
    }

    async fn push(
        &self,
        request: ImagePushRequest,
    ) -> Result<ImagePushResult, ContainerRuntimeError> {
        validate_push_request(&request)?;
        let program = self.config().program.clone();
        let digest_path = push_digest_path();
        let pushed = tokio::time::timeout(
            Duration::from_secs(request.timeout_secs),
            async_podman_command(&program)
                .arg("push")
                .arg("--digestfile")
                .arg(&digest_path)
                .arg(&request.local_reference)
                .arg(&request.remote_reference)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        let result = command_output("push", pushed);
        let digest_text = std::fs::read_to_string(&digest_path);
        let _ = std::fs::remove_file(&digest_path);
        let output = result?;
        if !output.status.success() {
            return Err(ContainerRuntimeError::Invalid(format!(
                "`{} push` failed with status {}: {}",
                program,
                output.status.code().unwrap_or(1),
                crate::connection::truncate_captured_bytes(&output.stderr)
            )));
        }
        let text = digest_text.map_err(|source| {
            ContainerRuntimeError::Invalid(format!(
                "cannot read push digest file {}: {source}",
                digest_path.display()
            ))
        })?;
        let digest = normalize_digest(&text).map_err(|error| {
            ContainerRuntimeError::Invalid(format!(
                "push digest file {}: {error}",
                digest_path.display()
            ))
        })?;
        Ok(ImagePushResult {
            remote_reference: request.remote_reference,
            digest,
        })
    }
}

/// Flatten a timeout/spawn/result triple into one `Output`.
fn command_output(
    command: &str,
    outcome: Result<Result<std::process::Output, std::io::Error>, tokio::time::error::Elapsed>,
) -> Result<std::process::Output, ContainerRuntimeError> {
    match outcome {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(ContainerRuntimeError::Invalid(format!(
            "cannot spawn podman {command}: {error}"
        ))),
        Err(_) => Err(ContainerRuntimeError::Timeout {
            timeout_secs: DEFAULT_PUSH_TIMEOUT_SECS,
        }),
    }
}

/// Render `podman build` argv (context last) for a validated request.
pub fn build_image_args(request: &ImageBuildRequest) -> Result<Vec<String>, ContainerRuntimeError> {
    validate_build_request(request)?;
    let mut args = vec![
        "--file".into(),
        request.containerfile.clone(),
        "--tag".into(),
        request.tag.clone(),
    ];
    for (name, value) in &request.build_args {
        args.push("--build-arg".into());
        args.push(format!("{name}={value}"));
    }
    args.push(request.context_dir.to_string_lossy().into_owned());
    Ok(args)
}

fn validate_build_request(request: &ImageBuildRequest) -> Result<(), ContainerRuntimeError> {
    if !request.context_dir.is_absolute() || !request.context_dir.is_dir() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "build context `{}` must be an existing absolute directory",
            request.context_dir.display()
        )));
    }
    if !is_safe_relative_path(&request.containerfile) {
        return Err(ContainerRuntimeError::Invalid(format!(
            "containerfile `{}` must be a context-relative path without traversal",
            request.containerfile
        )));
    }
    validate_reference_token(&request.tag, "tag")?;
    for (name, value) in &request.build_args {
        validate_reference_token(name, "build-arg name")?;
        if value.contains('\0') {
            return Err(ContainerRuntimeError::Invalid(
                "build-arg values cannot contain NUL".into(),
            ));
        }
    }
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn validate_push_request(request: &ImagePushRequest) -> Result<(), ContainerRuntimeError> {
    validate_reference_token(&request.local_reference, "local reference")?;
    validate_reference_token(&request.remote_reference, "remote reference")?;
    if request.timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn validate_reference_token(value: &str, kind: &str) -> Result<(), ContainerRuntimeError> {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ContainerRuntimeError::Invalid(format!(
            "{kind} must be nonempty and free of whitespace"
        )));
    }
    Ok(())
}

fn is_safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.contains('\0')
        && Path::new(value).components().all(|component| {
            matches!(component, std::path::Component::Normal(_))
                && component.as_os_str() != ".."
                && component.as_os_str() != "."
        })
}

/// Parse `podman inspect --format '{{.Id}}\n{{.Digest}}'` output into two
/// canonical `sha256:<hex>` values.
pub fn parse_inspect_id_digest(output: &str) -> Result<(String, String), String> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let image_id = normalize_digest(lines.next().ok_or("inspect output is empty")?)?;
    let digest = normalize_digest(
        lines
            .next()
            .ok_or("inspect output is missing the image digest")?,
    )?;
    Ok((image_id, digest))
}

/// Canonicalize `sha256:<64 hex>`, accepting an optional prefix (any casing)
/// and any hex casing (same bytes, same digest).
pub fn normalize_digest(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let hex = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(format!("sha256:{}", hex.to_ascii_lowercase()))
    } else {
        Err(format!(
            "digest must be `sha256:` + 64 hex characters, got `{trimmed}`"
        ))
    }
}

fn push_digest_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "autonomics-image-push-{}-{nanos}-{sequence}.digest",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> ImageBuildRequest {
        let context = tempfile::tempdir().unwrap();
        ImageBuildRequest {
            context_dir: context.keep(),
            containerfile: "Containerfile".into(),
            tag: "localhost/auto-nomics/environments/demo-env:rsi-1".into(),
            build_args: vec![("VERSION".into(), "4.5".into())],
            timeout_secs: 900,
        }
    }

    #[test]
    fn build_args_render_options_before_the_context() {
        let request = request();
        let args = build_image_args(&request).unwrap();
        let expected: Vec<String> = vec![
            "--file".into(),
            "Containerfile".into(),
            "--tag".into(),
            "localhost/auto-nomics/environments/demo-env:rsi-1".into(),
            "--build-arg".into(),
            "VERSION=4.5".into(),
            request.context_dir.to_string_lossy().into_owned(),
        ];
        assert_eq!(args, expected);
    }

    #[test]
    fn build_requests_reject_traversal_and_missing_contexts() {
        let mut traversal = request();
        traversal.containerfile = "../escape".into();
        assert!(build_image_args(&traversal).is_err());

        let mut absolute = request();
        absolute.containerfile = "/etc/passwd".into();
        assert!(build_image_args(&absolute).is_err());

        let mut missing = request();
        missing.context_dir = PathBuf::from("/nonexistent-context-dir");
        assert!(build_image_args(&missing).is_err());

        let mut blank_tag = request();
        blank_tag.tag = " ".into();
        assert!(build_image_args(&blank_tag).is_err());

        let mut zero_timeout = request();
        zero_timeout.timeout_secs = 0;
        assert!(build_image_args(&zero_timeout).is_err());
    }

    #[test]
    fn push_requests_validate_references_and_timeout() {
        let valid = ImagePushRequest {
            local_reference: "localhost/auto-nomics/environments/demo-env@sha256:aaaaaaaa".into(),
            remote_reference: "ghcr.io/auto-nomics/environments/demo-env:aaaaaaaa".into(),
            timeout_secs: 60,
        };
        assert!(validate_push_request(&valid).is_ok());

        let mut spaced = valid.clone();
        spaced.remote_reference = "ghcr.io /demo".into();
        assert!(validate_push_request(&spaced).is_err());

        let mut zero = valid;
        zero.timeout_secs = 0;
        assert!(validate_push_request(&zero).is_err());
    }

    #[test]
    fn inspect_output_parses_into_canonical_digests() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let (image_id, parsed) =
            parse_inspect_id_digest(&format!("{}\n{}\n", digest, digest.to_uppercase())).unwrap();
        assert_eq!(image_id, digest);
        assert_eq!(parsed, digest);
        assert!(parse_inspect_id_digest("").is_err());
        assert!(parse_inspect_id_digest(&digest).is_err());
    }

    #[test]
    fn digest_files_are_normalized_with_prefix_and_case_folding() {
        let digest = format!("sha256:{}", "cd".repeat(32));
        assert_eq!(normalize_digest(&digest).unwrap(), digest);
        assert_eq!(
            normalize_digest(digest.trim_start_matches("sha256:")).unwrap(),
            digest
        );
        assert_eq!(
            normalize_digest(&format!("{}\n", digest.to_uppercase())).unwrap(),
            digest
        );
        assert!(normalize_digest("sha256:short").is_err());
        assert!(normalize_digest(&format!("sha256:{}", "z".repeat(64))).is_err());
    }

    /// Verifies the real `podman build` + `inspect` command shapes against a
    /// digest-pinned alpine base. Push semantics need registry credentials
    /// and stay operator-verified (`--digestfile` re-pin).
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and pulls the alpine base image"]
    async fn real_podman_builds_and_inspects_a_containerfile() {
        let root = tempfile::tempdir().unwrap();
        let context = root.path().join("context");
        std::fs::create_dir_all(&context).unwrap();
        std::fs::write(
            context.join("Containerfile"),
            "FROM docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507\nRUN echo ok > /marker\n",
        )
        .unwrap();
        let runtime = PodmanRuntime::new(crate::podman::PodmanConfig {
            program: "podman".into(),
            workspace_root: root.path().join("workspace"),
            panel_cache_root: root.path().join("panels"),
        });
        let tag = "localhost/auto-nomics/environments/image-build-it:rsi-1".to_string();
        let result = runtime
            .build(ImageBuildRequest {
                context_dir: context,
                containerfile: "Containerfile".into(),
                tag: tag.clone(),
                build_args: Vec::new(),
                timeout_secs: 600,
            })
            .await
            .expect("real podman build");
        assert!(result.digest.starts_with("sha256:"));
        assert_eq!(result.digest.len(), "sha256:".len() + 64);
        // Best-effort cleanup of the local test tag.
        let _ = tokio::process::Command::new("podman")
            .args(["rmi", "--force", &tag])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}
