//! Site-supported, same-UID SIF execution inside an existing Slurm allocation.
//! This is an ordinary process backend, not a protected research runtime.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::process::{ProcessTaskSpec, render_process_script};
use crate::dag::DagError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApptainerMode {
    /// Execute the declared program directly, bypassing the image runscript.
    Exec,
    /// Invoke the converted OCI ENTRYPOINT / CMD runscript.
    Run,
}

/// Host networking is explicit: this profile provides no network denial proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApptainerNetwork {
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApptainerContainer {
    /// Full OCI reference including @sha256:<64 hex>; never a tag or SIF path.
    pub image: String,
    pub mode: ApptainerMode,
    pub network: ApptainerNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SifImage {
    /// Absolute compute-node path, managed by the operator outside task scratch.
    pub path: PathBuf,
    /// SHA256 of the actual SIF bytes, distinct from the OCI manifest digest.
    pub sha256: String,
}

/// Coordinator-side operator manifest. Task specs cannot select programs,
/// arbitrary bind mounts, privilege flags, or unregistered image paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApptainerSettings {
    pub program: PathBuf,
    pub images: BTreeMap<String, SifImage>,
}

fn invalid(message: &str) -> DagError {
    DagError::Schedule(format!("Apptainer: {message}"))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn image_reference(value: &str) -> bool {
    value.rsplit_once("@sha256:").is_some_and(|(name, hash)| {
        !name.is_empty()
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b':'))
            && digest(hash)
    })
}

fn absolute_path(path: &Path) -> Result<&str, DagError> {
    let value = path.to_str().ok_or_else(|| invalid("path must be UTF-8"))?;
    if !path.is_absolute()
        || path == Path::new("/")
        || value.contains([',', ':', '\\'])
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(invalid(
            "paths must be absolute, normalized and safe for bind syntax",
        ));
    }
    Ok(value)
}

impl ApptainerContainer {
    pub(super) fn validate(&self, process: &ProcessTaskSpec) -> Result<(), DagError> {
        if !image_reference(&self.image) {
            return Err(invalid(
                "container image requires a full pinned OCI reference",
            ));
        }
        if self.mode == ApptainerMode::Run && process.script.is_some() {
            return Err(invalid(
                "run mode requires command arguments; script requires exec mode",
            ));
        }
        for (name, value) in &process.env {
            if name.is_empty()
                || name.as_bytes()[0].is_ascii_digit()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || name.starts_with("APPTAINER")
                || name.starts_with("SINGULARITY")
                || name.starts_with("AUTONOMICS_")
                || matches!(name.as_str(), "HOME" | "TMPDIR" | "TMP" | "TEMP")
                || value.contains('\0')
            {
                return Err(invalid(
                    "invalid or reserved container environment variable",
                ));
            }
        }
        if process.command.iter().any(|arg| arg.contains('\0'))
            || process
                .script
                .as_ref()
                .is_some_and(|body| body.contains('\0'))
        {
            return Err(invalid("container payload contains NUL"));
        }
        Ok(())
    }
}

impl ApptainerSettings {
    pub fn load(path: &Path) -> Result<Self, DagError> {
        // Configuration is operator-owned; never read a remote task-provided file.
        let bytes =
            std::fs::read(path).map_err(|_| invalid("cannot read operator SIF manifest"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("operator SIF manifest exceeds 1 MiB"));
        }
        let settings: Self =
            serde_json::from_slice(&bytes).map_err(|_| invalid("invalid operator SIF manifest"))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), DagError> {
        absolute_path(&self.program)?;
        if self.images.is_empty() {
            return Err(invalid("operator SIF manifest has no images"));
        }
        for (reference, image) in &self.images {
            if !image_reference(reference)
                || !digest(&image.sha256)
                || image.path.extension().and_then(|s| s.to_str()) != Some("sif")
            {
                return Err(invalid(
                    "manifest requires full OCI references and SIF SHA256 values",
                ));
            }
            absolute_path(&image.path)?;
        }
        Ok(())
    }

    pub(super) fn resolve(&self, container: &ApptainerContainer) -> Result<&SifImage, DagError> {
        self.validate()?;
        self.images.get(&container.image).ok_or_else(|| {
            invalid("image is absent from the operator SIF manifest; automatic pulls are disabled")
        })
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// All inputs live below the existing task directory and keep their absolute
/// paths, so process input placeholders and the artifact protocol stay intact.
pub(super) fn render(
    settings: &ApptainerSettings,
    process: &ProcessTaskSpec,
    task_dir: &str,
    environment: &BTreeMap<String, String>,
    inputs: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<String, DagError> {
    let container = process
        .container
        .as_ref()
        .ok_or_else(|| invalid("missing container spec"))?;
    container.validate(process)?;
    let image = settings.resolve(container)?;
    absolute_path(Path::new(task_dir))?;
    if inputs
        .values()
        .flatten()
        .any(|path| absolute_path(path).is_err() || !path.starts_with(task_dir))
    {
        return Err(invalid(
            "container inputs must be staged inside the task workspace",
        ));
    }
    let runtime = format!("{task_dir}/.apptainer-runtime");
    let mut script = format!(
        "set -eu\numask 077\ncd {}\n\
         test -f {} && test ! -L {} || {{ echo 'SIF is missing or is a symlink' >&2; exit 78; }}\n\
         printf '%s  %s\\n' {} {} | /usr/bin/sha256sum --check --status || {{ echo 'SIF SHA256 mismatch' >&2; exit 78; }}\n\
         mkdir -m 700 -- {}\nmkdir -m 700 -- {}/home {}/tmp {}/cache {}/session\n",
        quote(task_dir),
        quote(image.path.to_str().unwrap()),
        quote(image.path.to_str().unwrap()),
        quote(&image.sha256),
        quote(image.path.to_str().unwrap()),
        quote(&runtime),
        quote(&runtime),
        quote(&runtime),
        quote(&runtime),
        quote(&runtime),
    );
    // Clear runtime control variables as well as payload environment. --cleanenv
    // alone would still honor ambient APPTAINER_BIND / APPTAINERENV_* settings.
    let mut args = vec![
        "/usr/bin/env".to_string(),
        "-i".into(),
        "PATH=/usr/bin:/bin".into(),
        "LANG=C".into(),
        format!("HOME={runtime}/home"),
        format!("TMPDIR={runtime}/tmp"),
        format!("APPTAINER_TMPDIR={runtime}/session"),
        format!("APPTAINER_CACHEDIR={runtime}/cache"),
        format!("APPTAINERENV_TMPDIR={runtime}/tmp"),
    ];
    args.extend(
        environment
            .iter()
            .map(|(name, value)| format!("APPTAINERENV_{name}={value}")),
    );
    args.push(settings.program.to_string_lossy().into_owned());
    args.push(
        match container.mode {
            ApptainerMode::Exec => "exec",
            ApptainerMode::Run => "run",
        }
        .into(),
    );
    args.extend([
        "--cleanenv".into(),
        "--no-eval".into(),
        "--contain".into(),
        "--home".into(),
        format!("{runtime}/home:{runtime}/home"),
        "--no-mount".into(),
        "cwd,hostfs,bind-paths".into(),
        "--bind".into(),
        format!("{task_dir}:{task_dir}:rw"),
        "--pwd".into(),
        task_dir.into(),
        "--workdir".into(),
        format!("{runtime}/session"),
        image.path.to_string_lossy().into_owned(),
    ]);
    if let Some(body) = &process.script {
        args.extend([
            "/bin/sh".into(),
            "-c".into(),
            render_process_script(body, environment, inputs),
        ]);
    } else {
        args.extend(
            process
                .command
                .iter()
                .map(|arg| render_process_script(arg, environment, inputs)),
        );
    }
    // Foreground only. Cancellation and resource/stop accounting remain Slurm's
    // responsibility; neither instances nor detached services are started here.
    script.push_str("exec ");
    script.push_str(
        &args
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    script.push('\n');
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, ApptainerSettings, ProcessTaskSpec) {
        let root = tempfile::tempdir().unwrap();
        let image = root.path().join("toy ' image.sif");
        std::fs::write(&image, b"synthetic SIF fixture").unwrap();
        let program = root.path().join("apptainer");
        // Model only argv / environment forwarding, not mount or UID isolation.
        std::fs::write(&program, r#"#!/usr/bin/python3
import json, os, sys
assert 'LEAKED_HOST_MARKER' not in os.environ
assert 'APPTAINER_BIND' not in os.environ
assert 'APPTAINERENV_UNDECLARED' not in os.environ
args = sys.argv[1:]
image = next(i for i, arg in enumerate(args) if arg.endswith('.sif'))
os.chdir(args[args.index('--pwd') + 1])
with open('invocation.json', 'w') as stream:
    json.dump({'args': args, 'env': dict(os.environ)}, stream)
environment = {'PATH': '/usr/bin:/bin'}
environment.update((k[len('APPTAINERENV_'):], v) for k, v in os.environ.items() if k.startswith('APPTAINERENV_'))
assert 'APPTAINERENV_HOME' not in os.environ
environment['HOME'] = args[args.index('--home') + 1].split(':')[-1]
payload = args[image + 1:]
if not payload:
    assert args[0] == 'run'
    payload = ['/bin/true']  # synthetic image CMD
os.execvpe(payload[0], payload, environment)
"#).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reference = format!("ghcr.io/autonomics/toy@sha256:{}", "a".repeat(64));
        let settings = ApptainerSettings {
            program,
            images: BTreeMap::from([(
                reference.clone(),
                SifImage {
                    path: image,
                    sha256: format!("{:x}", Sha256::digest(b"synthetic SIF fixture")),
                },
            )]),
        };
        let process = ProcessTaskSpec {
            container: Some(ApptainerContainer {
                image: reference,
                mode: ApptainerMode::Exec,
                network: ApptainerNetwork::Host,
            }),
            script: Some(
                "cat \"$AUTONOMICS_INPUT_F_0\" > result.txt\nprintf '%s' \"$DECLARED\" >> result.txt".into(),
            ),
            command: vec![],
            env: BTreeMap::from([(
                "DECLARED".into(),
                "literal ' $HOME $(touch INJECTED)".into(),
            )]),
            outputs: vec![],
        };
        (root, settings, process)
    }

    fn execute(
        root: &Path,
        settings: &ApptainerSettings,
        process: &ProcessTaskSpec,
    ) -> std::process::Output {
        let work = root.join("task");
        std::fs::create_dir_all(&work).unwrap();
        let input = work.join("input with ' quote");
        std::fs::write(&input, "synthetic input\n").unwrap();
        let inputs = BTreeMap::from([("f".into(), vec![input])]);
        let env = super::super::process::process_task_environment(
            "toy",
            work.to_str().unwrap(),
            &process.env,
            &inputs,
        )
        .unwrap();
        let script = render(settings, process, work.to_str().unwrap(), &env, &inputs).unwrap();
        std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(script)
            .env("LEAKED_HOST_MARKER", "must not reach runtime")
            .env("APPTAINER_BIND", "/host:/forbidden")
            .env("APPTAINERENV_UNDECLARED", "must not reach payload")
            .output()
            .unwrap()
    }

    #[test]
    fn exec_forwards_staged_input_literal_env_and_outputs_without_host_controls() {
        let (root, settings, process) = fixture();
        let result = execute(root.path(), &settings, &process);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let work = root.path().join("task");
        assert_eq!(
            std::fs::read_to_string(work.join("result.txt")).unwrap(),
            "synthetic input\nliteral ' $HOME $(touch INJECTED)"
        );
        assert!(!work.join("INJECTED").exists());
        let recorded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(work.join("invocation.json")).unwrap()).unwrap();
        let args = recorded["args"].as_array().unwrap();
        assert_eq!(args[0], "exec");
        for flag in ["--cleanenv", "--no-eval", "--contain", "--home", "--bind"] {
            assert!(args.iter().any(|v| v == flag), "{flag}");
        }
        assert!(
            !args
                .iter()
                .any(|v| v == "--fakeroot" || v == "--writable" || v == "--net")
        );
    }

    #[test]
    fn changed_sif_bytes_and_symlink_are_refused_before_launch() {
        for symlink in [false, true] {
            let (root, settings, process) = fixture();
            let image = &settings.images.values().next().unwrap().path;
            if symlink {
                let original = root.path().join("original.sif");
                std::fs::rename(image, &original).unwrap();
                std::os::unix::fs::symlink(original, image).unwrap();
            } else {
                std::fs::write(image, "changed synthetic bytes").unwrap();
            }
            let result = execute(root.path(), &settings, &process);
            assert_eq!(result.status.code(), Some(78));
            assert!(!root.path().join("task/invocation.json").exists());
        }
    }

    #[test]
    fn run_mode_is_explicit_and_does_not_accept_a_script() {
        let (root, settings, mut process) = fixture();
        process.container.as_mut().unwrap().mode = ApptainerMode::Run;
        assert!(process.validate().is_err());
        process.script = None;
        process.command = vec!["/bin/true".into()];
        process.validate().unwrap();
        assert!(execute(root.path(), &settings, &process).status.success());
        let recorded: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("task/invocation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(recorded["args"][0], "run");
        process.command.clear();
        process.validate().unwrap();
        assert!(execute(root.path(), &settings, &process).status.code() != Some(0));
        // A fresh workspace is needed: the runtime directory is exclusive.
        std::fs::remove_dir_all(root.path().join("task")).unwrap();
        assert!(execute(root.path(), &settings, &process).status.success());
        process.container.as_mut().unwrap().mode = ApptainerMode::Exec;
        assert!(process.validate().is_err());
    }

    #[test]
    fn manifest_requires_full_digests_and_normalized_absolute_sif_paths() {
        let (_root, settings, process) = fixture();
        settings.validate().unwrap();
        for path in [
            "~/images/toy.sif",
            "/images/../toy.sif",
            "/images/toy.sif:rw",
            "/images/toy.sif,other",
            "/images/toy.img",
        ] {
            let mut changed = settings.clone();
            changed.images.values_mut().next().unwrap().path = path.into();
            assert!(changed.validate().is_err(), "{path}");
        }
        let mut changed = settings.clone();
        changed.images.values_mut().next().unwrap().sha256 = "a".repeat(8);
        assert!(changed.validate().is_err());
        let mut absent = process.container.clone().unwrap();
        absent.image = format!("ghcr.io/autonomics/other@sha256:{}", "b".repeat(64));
        assert!(settings.resolve(&absent).is_err());
        for image in ["toy:latest", "/images/toy.sif", "toy@sha256:12345678"] {
            absent.image = image.into();
            assert!(absent.validate(&process).is_err());
        }
    }

    #[test]
    fn reserved_environment_and_unstaged_inputs_are_rejected() {
        let (_root, settings, process) = fixture();
        for name in [
            "APPTAINER_BIND",
            "SINGULARITYENV_HOME",
            "AUTONOMICS_TASK_ID",
            "HOME",
            "TMPDIR",
            "BAD-NAME",
            "1BAD",
        ] {
            let mut changed = process.clone();
            changed.env.insert(name.into(), "value".into());
            assert!(changed.validate().is_err(), "{name}");
        }
        assert!(
            render(
                &settings,
                &process,
                "/scratch/task",
                &BTreeMap::new(),
                &BTreeMap::from([("f".into(), vec![PathBuf::from("/scratch/other/input")])])
            )
            .is_err()
        );
        assert!(
            render(
                &settings,
                &process,
                "/scratch/task",
                &BTreeMap::new(),
                &BTreeMap::from([(
                    "f".into(),
                    vec![PathBuf::from("/scratch/task/../other/input")]
                )]),
            )
            .is_err()
        );
        for raw in [
            r#"{"image":"toy","mode":"exec","network":"isolated"}"#,
            r#"{"image":"toy","mode":"exec"}"#,
            r#"{"image":"toy","mode":"exec","network":"host","fakeroot":true}"#,
        ] {
            assert!(serde_json::from_str::<ApptainerContainer>(raw).is_err());
        }
    }
}
