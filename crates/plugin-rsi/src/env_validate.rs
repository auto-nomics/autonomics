//! Deterministic validation gates for environment development workspaces.
//!
//! Gates 1–4 are pure static analysis over the workspace. Gates 5–6 exercise
//! daemon-owned infrastructure (build, then smoke tests inside the freshly
//! built image); a missing builder is `blocked`, never a silent pass, because
//! missing infrastructure is not evidence of correctness.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use container_runtime::{
    ContainerNetwork, ContainerRunRequest, GpuRequest, ImageBuildConnection, ImageBuildRequest,
    PodmanConnection, PullPolicy, unique_container_name,
};
use serde::{Deserialize, Serialize};

use crate::{
    Error, GateResult, GateStatus, PluginWorkspace, Result,
    env_manifest::{EnvironmentManifest, validate_interpreters},
    request::unix_now,
};

/// Which gates a validation run includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationDepth {
    /// The four deterministic static gates only; no build, no smoke run.
    Static,
    /// All six gates, including the infrastructure build and smoke tests.
    Full,
}

/// Budget for one smoke-test container.
const SMOKE_TIMEOUT_SECS: u64 = 600;
/// Resource shape mirrors `plugin_container_run`: smoke tests are debugging
/// probes, not data-plane workloads.
const SMOKE_CPUS: f64 = 2.0;
const SMOKE_MEMORY: &str = "1g";
const SMOKE_PIDS_LIMIT: i64 = 256;
const SMOKE_SHM_SIZE: &str = "256m";

/// Deterministic report of one environment validation attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentValidationReport {
    pub report_id: String,
    pub environment_id: String,
    pub created_at: i64,
    pub overall: GateStatus,
    pub gates: Vec<GateResult>,
    /// Local tag of the image produced by the build gate, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_tag: Option<String>,
    /// Local manifest digest of the built image, when the build gate passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl EnvironmentValidationReport {
    pub fn new(
        environment_id: &str,
        attempt: u32,
        gates: Vec<GateResult>,
        local_tag: Option<String>,
        digest: Option<String>,
    ) -> Self {
        let overall = if gates.iter().any(|gate| gate.status == GateStatus::Fail) {
            GateStatus::Fail
        } else if gates.iter().any(|gate| gate.status == GateStatus::Blocked) {
            GateStatus::Blocked
        } else {
            GateStatus::Pass
        };
        Self {
            report_id: format!("attempt-{attempt}"),
            environment_id: environment_id.to_string(),
            created_at: unix_now(),
            overall,
            gates,
            local_tag,
            digest,
        }
    }

    pub fn passed(&self) -> bool {
        self.overall == GateStatus::Pass
    }

    /// Append-only persistence: an agent cannot replace prior evidence.
    pub fn write(&self, reports_dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(reports_dir)?;
        let path = reports_dir.join(format!("{}.json", self.report_id));
        if path.exists() {
            return Err(Error::Validation(format!(
                "report {} already exists",
                path.display()
            )));
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| Error::Validation(error.to_string()))?;
        let tmp = reports_dir.join(format!(".{}.tmp", self.report_id));
        std::fs::write(&tmp, text).map_err(|source| Error::WriteFile {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::WriteFile {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

/// Run the deterministic environment gates over one workspace.
///
/// [`ValidationDepth::Static`] runs the four pure gates and stops. A
/// [`ValidationDepth::Full`] run without a builder reports the build gate as
/// `blocked` — missing infrastructure is not evidence of correctness — and
/// with a builder it constructs the local tag
/// `localhost/<namespace>/<id>:rsi-<attempt>` and smoke-tests the image.
pub async fn validate_environment(
    environment_id: &str,
    workspace: &PluginWorkspace,
    local_namespace: &str,
    depth: ValidationDepth,
    builder: Option<&dyn ImageBuildConnection>,
    runner: Option<&dyn PodmanConnection>,
    attempt: u32,
) -> EnvironmentValidationReport {
    let mut gates: Vec<GateResult> = Vec::new();
    let manifest = run_gate(&mut gates, "manifest", || {
        let text = workspace
            .read_text("manifest.toml")
            .map_err(|error| error.to_string())?;
        let manifest = parse_manifest_text(&text)?;
        check_manifest(&manifest, workspace, environment_id)?;
        Ok(manifest)
    });
    let Some(manifest) = manifest else {
        return blocked_rest(environment_id, attempt, gates, "manifest");
    };

    let files = match workspace.list_files() {
        Ok(files) => files,
        Err(error) => {
            gates.push(GateResult::fail("base_policy", error.to_string()));
            return blocked_rest(environment_id, attempt, gates, "base_policy");
        }
    };

    match gate_base_policy(&manifest) {
        Ok(()) => gates.push(GateResult::pass("base_policy")),
        Err(error) => {
            gates.push(GateResult::fail("base_policy", error));
            return blocked_rest(environment_id, attempt, gates, "base_policy");
        }
    }

    let containerfile = match workspace.read_text(&manifest.containerfile) {
        Ok(text) => text,
        Err(error) => {
            gates.push(GateResult::fail("containerfile_static", error.to_string()));
            return blocked_rest(environment_id, attempt, gates, "containerfile_static");
        }
    };
    match gate_containerfile_static(&containerfile, &files) {
        Ok(()) => gates.push(GateResult::pass("containerfile_static")),
        Err(error) => {
            gates.push(GateResult::fail("containerfile_static", error));
            return blocked_rest(environment_id, attempt, gates, "containerfile_static");
        }
    }

    match gate_secret_scan(workspace, &files) {
        Ok(()) => gates.push(GateResult::pass("secret_scan")),
        Err(error) => {
            gates.push(GateResult::fail("secret_scan", error));
            // A credential must never be baked into an image.
            return blocked_rest(environment_id, attempt, gates, "secret_scan");
        }
    }

    if depth == ValidationDepth::Static {
        return EnvironmentValidationReport::new(environment_id, attempt, gates, None, None);
    }

    let tag = format!("localhost/{local_namespace}/{environment_id}:rsi-{attempt}");
    let Some(builder) = builder else {
        gates.push(GateResult::blocked(
            "build",
            "image builder is not configured",
        ));
        gates.push(GateResult::blocked("smoke", "not run: no image was built"));
        return EnvironmentValidationReport::new(environment_id, attempt, gates, None, None);
    };
    let built = builder
        .build(ImageBuildRequest {
            context_dir: workspace.path().to_path_buf(),
            containerfile: manifest.containerfile.clone(),
            tag: tag.clone(),
            build_args: Vec::new(),
            timeout_secs: DEFAULT_BUILD_TIMEOUT_SECS,
        })
        .await;
    let digest = match built {
        Ok(result) => {
            gates.push(GateResult::pass("build"));
            result.digest
        }
        Err(error) => {
            gates.push(GateResult::fail("build", error.to_string()));
            gates.push(GateResult::blocked("smoke", "not run after `build` failed"));
            return EnvironmentValidationReport::new(environment_id, attempt, gates, None, None);
        }
    };

    let Some(runner) = runner else {
        gates.push(GateResult::blocked(
            "smoke",
            "container runner is not configured",
        ));
        return EnvironmentValidationReport::new(
            environment_id,
            attempt,
            gates,
            Some(tag),
            Some(digest),
        );
    };
    let smoke = gate_smoke(runner, &manifest, &tag).await;
    match smoke {
        Ok(()) => gates.push(GateResult::pass("smoke")),
        Err(error) => gates.push(GateResult::fail("smoke", error)),
    }
    EnvironmentValidationReport::new(environment_id, attempt, gates, Some(tag), Some(digest))
}

/// Budget for one environment image build (base pull plus layer assembly).
pub const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 3600;

/// Parse manifest text into an [`EnvironmentManifest`].
///
/// Shared by the `manifest` gate and the `environment_manifest_update` tool
/// so both accept exactly the same grammar (including `deny_unknown_fields`).
pub(crate) fn parse_manifest_text(text: &str) -> std::result::Result<EnvironmentManifest, String> {
    toml::from_str(text).map_err(|error| format!("invalid environment manifest: {error}"))
}

/// Structural checks for one parsed manifest against its workspace.
///
/// Everything after parsing in the `manifest` gate: identity must match the
/// workspace, interpreters must be well-formed, the declared containerfile
/// must exist, and smoke tests must be nonempty and executable-shaped.
pub(crate) fn check_manifest(
    manifest: &EnvironmentManifest,
    workspace: &PluginWorkspace,
    environment_id: &str,
) -> std::result::Result<(), String> {
    if manifest.environment_id != environment_id {
        return Err(format!(
            "manifest environment_id `{}` does not match workspace `{environment_id}`",
            manifest.environment_id
        ));
    }
    if workspace.path().file_name().and_then(|name| name.to_str()) != Some(environment_id) {
        return Err(format!(
            "workspace directory must be named `{environment_id}`"
        ));
    }
    validate_interpreters(&manifest.interpreters).map_err(|error| error.to_string())?;
    if manifest.containerfile.is_empty() {
        return Err("manifest must declare a containerfile".into());
    }
    if !is_safe_relative(&manifest.containerfile) {
        return Err(format!(
            "containerfile `{}` must be a workspace-relative path",
            manifest.containerfile
        ));
    }
    let files = workspace.list_files().map_err(|error| error.to_string())?;
    if !files.iter().any(|file| file == &manifest.containerfile) {
        return Err(format!(
            "containerfile `{}` does not exist in the workspace",
            manifest.containerfile
        ));
    }
    if manifest.tests.is_empty() {
        return Err("manifest must declare at least one smoke test".into());
    }
    for test in &manifest.tests {
        if test.name.trim().is_empty() {
            return Err("smoke tests must have a name".into());
        }
        if test.argv.is_empty() || test.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(format!(
                "smoke test `{}` must declare a nonempty NUL-free argv",
                test.name
            ));
        }
    }
    Ok(())
}

pub(crate) fn gate_base_policy(
    manifest: &crate::env_manifest::EnvironmentManifest,
) -> std::result::Result<(), String> {
    container_runtime::ImageReference::parse(&manifest.base.reference)
        .map(|_| ())
        .map_err(|error| format!("base reference is not digest-pinned: {error}"))
}

/// Static review of the Containerfile.
///
/// Every `FROM` must be digest-pinned; remote `ADD`, pipes into shells,
/// wildcard or missing `COPY` sources, absolute host-context sources, and
/// host-path tokens inside `RUN` are rejected.
fn gate_containerfile_static(text: &str, files: &[String]) -> std::result::Result<(), String> {
    let file_set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    for instruction in parse_instructions(text) {
        match instruction.keyword.as_str() {
            "FROM" => {
                let reference = from_reference(&instruction.args)
                    .ok_or("FROM instruction does not name an image".to_string())?;
                if reference == "scratch" {
                    return Err(
                        "`FROM scratch` is not allowed; base images must be digest-pinned".into(),
                    );
                }
                container_runtime::ImageReference::parse(&reference)
                    .map(|_| ())
                    .map_err(|error| format!("FROM reference is not digest-pinned: {error}"))?;
            }
            "RUN" => {
                if pipes_into_shell(&instruction.args) {
                    return Err(format!(
                        "line {}: RUN must not pipe into a shell (`| sh`/`| bash`)",
                        instruction.line
                    ));
                }
                for forbidden in ["/home/", "/Users/", "/mnt/"] {
                    if instruction.args.contains(forbidden) {
                        return Err(format!(
                            "line {}: RUN contains forbidden host-path token {forbidden:?}",
                            instruction.line
                        ));
                    }
                }
            }
            "COPY" | "ADD" => {
                let (from_stage, sources) = copy_sources(&instruction);
                for source in sources {
                    if source.starts_with("http://") || source.starts_with("https://") {
                        return Err(format!(
                            "line {}: remote ADD/COPY sources are not allowed ({source})",
                            instruction.line
                        ));
                    }
                    if source.contains('*') || source.contains('?') {
                        return Err(format!(
                            "line {}: wildcard COPY sources are not allowed; list files \
                             explicitly ({source})",
                            instruction.line
                        ));
                    }
                    if source.starts_with('/') {
                        if !from_stage {
                            return Err(format!(
                                "line {}: COPY source `{source}` must be workspace-relative",
                                instruction.line
                            ));
                        }
                        // A `--from=<stage>` source addresses the build stage,
                        // not the host context.
                        continue;
                    }
                    if !is_safe_relative(&source) {
                        return Err(format!(
                            "line {}: COPY source `{source}` escapes the workspace",
                            instruction.line
                        ));
                    }
                    let known = file_set.contains(source.as_str())
                        || files
                            .iter()
                            .any(|file| file.starts_with(&format!("{source}/")));
                    if !known {
                        return Err(format!(
                            "line {}: COPY source `{source}` does not exist in the workspace",
                            instruction.line
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn gate_smoke(
    runner: &dyn PodmanConnection,
    manifest: &crate::env_manifest::EnvironmentManifest,
    tag: &str,
) -> std::result::Result<(), String> {
    let scratch = runner.workspace_root().join("environment-smoke");
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|error| format!("cannot prepare smoke scratch: {error}"))?;

    let mut probes: Vec<(String, Vec<String>, Option<String>)> = manifest
        .tests
        .iter()
        .map(|test| {
            (
                test.name.clone(),
                test.argv.clone(),
                test.expected_stdout_contains.clone(),
            )
        })
        .collect();
    // Close the honor-system gap: every declared interpreter must actually
    // resolve inside the image.
    for interpreter in &manifest.interpreters {
        probes.push((
            format!("interpreter-probe:{interpreter}"),
            vec![
                "sh".to_string(),
                "-eu".to_string(),
                "-c".to_string(),
                format!("command -v {interpreter}"),
            ],
            None,
        ));
    }

    for (name, argv, expected) in probes {
        let request = ContainerRunRequest {
            image: tag.to_string(),
            command: argv,
            workspace: container_runtime::trusted_workspace_ref(&scratch, "/work")
                .map_err(|error| error.to_string())?,
            env: Vec::new(),
            panels: Vec::new(),
            input_mounts: Vec::new(),
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(SMOKE_CPUS),
            memory: Some(SMOKE_MEMORY.into()),
            pids_limit: Some(SMOKE_PIDS_LIMIT),
            shm_size: Some(SMOKE_SHM_SIZE.into()),
            gpus: GpuRequest::None,
            user: None,
            timeout_secs: SMOKE_TIMEOUT_SECS,
            name: unique_container_name(),
        };
        match runner.run(request).await {
            Ok(output) => {
                if let Some(expected) = expected
                    && !output.stdout.contains(&expected)
                {
                    return Err(format!(
                        "smoke test `{name}` stdout did not contain {expected:?} \
                         (stdout: {})",
                        truncate(&output.stdout)
                    ));
                }
            }
            Err(container_runtime::ContainerRuntimeError::ExitStatus {
                exit_code, stderr, ..
            }) => {
                return Err(format!(
                    "smoke test `{name}` exited with status {exit_code}: {}",
                    truncate(&stderr)
                ));
            }
            Err(error) => {
                return Err(format!("smoke test `{name}` could not run: {error}"));
            }
        }
    }
    Ok(())
}

fn gate_secret_scan(
    workspace: &PluginWorkspace,
    files: &[String],
) -> std::result::Result<(), String> {
    for path in files {
        let text = workspace
            .read_text(path)
            .map_err(|error| error.to_string())?;
        for marker in ["ghp_", "github_pat_", "AKIA", "BEGIN PRIVATE KEY"] {
            if text.contains(marker) {
                return Err(format!("possible credential in {path}"));
            }
        }
    }
    Ok(())
}

struct Instruction {
    keyword: String,
    args: String,
    line: usize,
}

/// Split a Containerfile into instructions, joining `\` continuations and
/// dropping comments.
fn parse_instructions(text: &str) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut buffer = String::new();
    let mut buffer_line = 0;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if buffer.is_empty() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            buffer_line = index + 1;
        }
        if let Some(head) = line.strip_suffix('\\') {
            buffer.push_str(head.trim_end());
            buffer.push(' ');
        } else {
            buffer.push_str(line);
            let (keyword, args) = buffer
                .split_once(char::is_whitespace)
                .unwrap_or((buffer.as_str(), ""));
            instructions.push(Instruction {
                keyword: keyword.to_ascii_uppercase(),
                args: args.trim().to_string(),
                line: buffer_line,
            });
            buffer.clear();
        }
    }
    instructions
}

fn from_reference(args: &str) -> Option<String> {
    args.split_whitespace()
        .find(|token| !token.starts_with("--"))
        .map(str::to_string)
}

/// Split COPY/ADD args into `--from` presence and the source operands.
fn copy_sources(instruction: &Instruction) -> (bool, Vec<String>) {
    let mut from_stage = false;
    let mut operands = Vec::new();
    for token in instruction.args.split_whitespace() {
        if token.starts_with("--") {
            if token.starts_with("--from") {
                from_stage = true;
            }
            continue;
        }
        operands.push(token.to_string());
    }
    // The final operand is the destination; only sources are validated.
    if operands.len() >= 2 {
        operands.pop();
    }
    (from_stage, operands)
}

fn pipes_into_shell(args: &str) -> bool {
    const SHELLS: [&str; 5] = ["sh", "bash", "ash", "dash", "zsh"];
    let tokens: Vec<&str> = args.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        if *token == "|"
            && let Some(next) = tokens.get(index + 1)
            && SHELLS.contains(next)
        {
            return true;
        }
        // Pipes glued to the previous word (`curl … |sh`) split into segments.
        for segment in token.split('|').skip(1) {
            if SHELLS.contains(&segment) {
                return true;
            }
        }
    }
    false
}

fn is_safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.contains('\0')
        && std::path::Path::new(value)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 512;
    if value.len() <= LIMIT {
        return value.to_string();
    }
    format!("{}…[truncated]", &value[..LIMIT])
}

fn run_gate<T, F>(gates: &mut Vec<GateResult>, name: &str, operation: F) -> Option<T>
where
    F: FnOnce() -> std::result::Result<T, String>,
{
    match operation() {
        Ok(value) => {
            gates.push(GateResult::pass(name));
            Some(value)
        }
        Err(error) => {
            gates.push(GateResult::fail(name, error));
            None
        }
    }
}

fn blocked_rest(
    environment_id: &str,
    attempt: u32,
    mut gates: Vec<GateResult>,
    failed_gate: &str,
) -> EnvironmentValidationReport {
    for name in [
        "base_policy",
        "containerfile_static",
        "secret_scan",
        "build",
        "smoke",
    ] {
        if gates.iter().any(|gate| gate.name == name) {
            continue;
        }
        gates.push(GateResult::blocked(
            name,
            format!("not run after `{failed_gate}` failed"),
        ));
    }
    EnvironmentValidationReport::new(environment_id, attempt, gates, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_join_continuations_and_drop_comments() {
        let text = "# comment\nFROM debian@sha256:x AS base\nRUN apt-get update && \\\n    apt-get install -y curl\n";
        let instructions = parse_instructions(text);
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].keyword, "FROM");
        assert_eq!(instructions[0].line, 2);
        assert_eq!(
            instructions[1].args,
            "apt-get update && apt-get install -y curl"
        );
    }

    #[test]
    fn pipes_into_shell_detects_spaced_and_compact_forms() {
        assert!(pipes_into_shell("curl https://x | sh"));
        assert!(pipes_into_shell("curl https://x|sh"));
        assert!(pipes_into_shell("wget -qO- https://x | bash -s"));
        assert!(!pipes_into_shell("echo one | grep two"));
        assert!(!pipes_into_shell("cat list|sort"));
    }

    #[test]
    fn from_reference_skips_platform_flags() {
        assert_eq!(
            from_reference("--platform=linux/amd64 debian@sha256:x AS base"),
            Some("debian@sha256:x".into())
        );
        assert_eq!(from_reference("scratch"), Some("scratch".into()));
    }

    #[test]
    fn copy_sources_split_flags_and_destination() {
        let instruction = Instruction {
            keyword: "COPY".into(),
            args: "--from=builder --chown=root:root /out/data /data/".into(),
            line: 3,
        };
        let (from_stage, sources) = copy_sources(&instruction);
        assert!(from_stage);
        assert_eq!(sources, vec!["/out/data".to_string()]);
    }

    #[test]
    fn containerfile_gate_rejects_each_documented_hazard() {
        let pinned = "docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507";
        let files = vec!["Containerfile".to_string(), "assets/notes.txt".to_string()];

        let ok = format!("FROM {pinned}\nCOPY assets/notes.txt /usr/share/notes.txt\n");
        assert!(gate_containerfile_static(&ok, &files).is_ok(), "{ok}");

        let unpinned = format!("FROM docker.io/library/alpine:latest\n");
        assert!(gate_containerfile_static(&unpinned, &files).is_err());

        let scratch = "FROM scratch\n".to_string();
        assert!(gate_containerfile_static(&scratch, &files).is_err());

        let remote_add = format!("FROM {pinned}\nADD https://example.com/data.tsv /data.tsv\n");
        assert!(gate_containerfile_static(&remote_add, &files).is_err());

        let piped = format!("FROM {pinned}\nRUN curl https://x | sh\n");
        assert!(gate_containerfile_static(&piped, &files).is_err());

        let missing = format!("FROM {pinned}\nCOPY assets/absent.txt /absent.txt\n");
        assert!(gate_containerfile_static(&missing, &files).is_err());

        let wildcard = format!("FROM {pinned}\nCOPY assets/* /assets/\n");
        assert!(gate_containerfile_static(&wildcard, &files).is_err());

        let absolute = format!("FROM {pinned}\nCOPY /etc/passwd /passwd\n");
        assert!(gate_containerfile_static(&absolute, &files).is_err());

        let host_path = format!("FROM {pinned}\nRUN cp /mnt/data/x /usr/bin/x\n");
        assert!(gate_containerfile_static(&host_path, &files).is_err());

        let traversal = format!("FROM {pinned}\nCOPY ../escape.txt /escape.txt\n");
        assert!(gate_containerfile_static(&traversal, &files).is_err());

        let stage_copy =
            format!("FROM {pinned} AS base\nFROM {pinned}\nCOPY --from=base /out /in\n");
        assert!(gate_containerfile_static(&stage_copy, &files).is_ok());

        let directory_source = format!("FROM {pinned}\nCOPY assets /assets/\n");
        assert!(gate_containerfile_static(&directory_source, &files).is_ok());
    }

    #[test]
    fn report_aggregates_and_is_append_only() {
        let tmp = tempfile::tempdir().unwrap();
        let report = EnvironmentValidationReport::new(
            "demo-env",
            1,
            vec![
                GateResult::pass("manifest"),
                GateResult::blocked("build", "image builder is not configured"),
            ],
            None,
            None,
        );
        assert_eq!(report.overall, GateStatus::Blocked);
        let path = report.write(&tmp.path().join("reports")).unwrap();
        assert!(path.is_file());
        assert!(report.write(&tmp.path().join("reports")).is_err());
    }
}
