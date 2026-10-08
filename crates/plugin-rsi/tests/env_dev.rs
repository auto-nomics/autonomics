//! Environment (image) development lifecycle: create → edit Containerfile →
//! manifest update → validate → build → local activation → background
//! publication → rollback → uninstall, plus static-gate rejections, blocked
//! infrastructure, dirty-tree refusal, and lifecycle guards.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

mod common;

use agentik_core::tools::{ToolError, ToolRegistration, ToolResult};
use agentik_sdk::types::ToolResultContent;
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, ImageBuildConnection,
    ImageBuildRequest, ImageBuildResult, ImagePushRequest, ImagePushResult, PodmanConnection,
};
use plugin_rsi::{
    Environment, EnvironmentCatalog, EnvironmentDevInfra, EnvironmentDistiller,
    EnvironmentRegistry, EnvironmentStatus, GateStatus, PluginDevelopmentToolsetRegistry,
    PublishedImageReference, RequestIntent, RequestRecord, RequestSource, RequestStatus,
};
use serde_json::{Value, json};

const BASE_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";
const REMOTE_DIGEST: &str =
    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// The smoke-test block `EnvironmentStore::create` seeds into every fresh
/// manifest.toml, exactly as `toml::to_string_pretty` renders it.
const DEFAULT_TESTS_BLOCK: &str = "[[tests]]\nname = \"base-interpreters\"\nargv = [\"true\"]";

/// The process-global toolset registry is shared state; environment tests
/// reconfigure it per test, so the tests in this binary run sequentially.
static SEQUENTIAL: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct FakeImageBuilder {
    builds: Mutex<Vec<ImageBuildRequest>>,
    pushes: Mutex<Vec<ImagePushRequest>>,
    counter: AtomicU64,
}

impl FakeImageBuilder {
    fn build_count(&self) -> usize {
        self.builds.lock().unwrap().len()
    }
}

#[async_trait]
impl ImageBuildConnection for FakeImageBuilder {
    async fn build(
        &self,
        request: ImageBuildRequest,
    ) -> Result<ImageBuildResult, ContainerRuntimeError> {
        assert!(request.context_dir.is_absolute());
        assert!(
            request
                .tag
                .starts_with("localhost/auto-nomics/environments/")
        );
        self.builds.lock().unwrap().push(request);
        let digest = format!(
            "sha256:{:064x}",
            self.counter.fetch_add(1, Ordering::Relaxed) + 1
        );
        Ok(ImageBuildResult {
            image_id: digest.clone(),
            digest,
        })
    }

    async fn push(
        &self,
        request: ImagePushRequest,
    ) -> Result<ImagePushResult, ContainerRuntimeError> {
        let remote = request.remote_reference.clone();
        self.pushes.lock().unwrap().push(request);
        Ok(ImagePushResult {
            remote_reference: remote,
            digest: REMOTE_DIGEST.into(),
        })
    }
}

struct FakeRunner {
    root: PathBuf,
}

#[async_trait]
impl PodmanConnection for FakeRunner {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        if request.command.iter().any(|argument| argument == "boom") {
            return Err(ContainerRuntimeError::ExitStatus {
                exit_code: 3,
                stderr: "fake smoke failure".into(),
                stdout: String::new(),
            });
        }
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: format!("ran: {}\n", request.command.join(" ")),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake"
    }

    fn workspace_root(&self) -> &Path {
        &self.root
    }
}

#[derive(Default)]
struct FakePublisher;

#[async_trait]
impl plugin_rsi::ImagePublisher for FakePublisher {
    async fn push_environment(
        &self,
        environment_id: &str,
        local_reference: &str,
    ) -> plugin_rsi::Result<PublishedImageReference> {
        if !local_reference.contains('@') {
            return Err(plugin_rsi::Error::ImageRegistry(
                "local reference must be digest-pinned".into(),
            ));
        }
        Ok(PublishedImageReference {
            reference: format!("ghcr.io/auto-nomics/environments/{environment_id}@{REMOTE_DIGEST}"),
            digest: REMOTE_DIGEST.into(),
        })
    }
}

struct Harness {
    state: tempfile::TempDir,
    infra: Arc<EnvironmentDevInfra>,
    registry: EnvironmentRegistry,
    builder: Arc<FakeImageBuilder>,
}

fn catalog() -> EnvironmentCatalog {
    let mut catalog = EnvironmentCatalog::default();
    catalog.insert(
        "alpine",
        Environment {
            reference: BASE_REFERENCE.into(),
            interpreters: vec!["sh".into()],
        },
    );
    catalog
}

fn environment_request(environment_id: &str, summary: &str) -> RequestRecord {
    RequestRecord {
        id: String::new(),
        created_at: 0,
        source: RequestSource::User,
        intent: RequestIntent::NewEnvironment,
        summary: summary.into(),
        body: "Provide the environment with the requested capability.".into(),
        plugin_name: Some(environment_id.into()),
        evidence_ids: Vec::new(),
        status: RequestStatus::Open,
    }
}

fn harness(with_builder: bool) -> Harness {
    let state = tempfile::tempdir().unwrap();
    common::configure_environment_vfs(state.path());
    let registry = EnvironmentRegistry::ephemeral(catalog());
    let builder = Arc::new(FakeImageBuilder::default());
    let publisher: plugin_rsi::SharedImagePublisher = Arc::new(FakePublisher);
    let runner = Arc::new(FakeRunner {
        root: state.path().join("smoke-root"),
    });
    let infra = Arc::new(
        EnvironmentDevInfra::open(
            state.path(),
            "main",
            "Autonomics RSI",
            "rsi@example.com",
            plugin_rsi::DEFAULT_LOCAL_NAMESPACE,
            registry.clone(),
            publisher,
        )
        .unwrap(),
    );
    if with_builder {
        infra.configure_builder(builder.clone());
        infra.configure_runner(runner.clone());
    }
    PluginDevelopmentToolsetRegistry::global()
        .configure_environment_dev(Arc::clone(&infra))
        .unwrap();
    PluginDevelopmentToolsetRegistry::global()
        .configure_runtime(runner)
        .unwrap();
    Harness {
        state,
        infra,
        registry,
        builder,
    }
}

async fn execute(
    tools: &[ToolRegistration],
    name: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    tools
        .iter()
        .find(|tool| tool.definition.name == name)
        .unwrap()
        .implementation
        .execute(input)
        .await
}

async fn json_result(tools: &[ToolRegistration], name: &str, input: Value) -> Value {
    let result = execute(tools, name, input).await.unwrap();
    let ToolResultContent::Json(value) = result.content else {
        panic!("tool `{name}` must return JSON");
    };
    value
}

#[tokio::test]
async fn environment_is_developed_activated_published_and_rolled_back() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(true);
    let infra = harness.infra.clone();
    let tools = plugin_rsi::environment_development_tool_registrations("env-dev-agent");
    let environment_path = "/environments/dev/bioconductor-extra";

    let operator = infra
        .create_environment(
            environment_request("bioconductor-extra", "Add extra Bioconductor packages"),
            "alpine",
        )
        .unwrap();
    assert_eq!(operator.status(), EnvironmentStatus::Draft);
    assert!(
        operator
            .workspace()
            .read_text("Containerfile")
            .unwrap()
            .starts_with("FROM ")
    );
    let request_id = operator.manifest().lifecycle.request_ids[0].clone();
    assert_eq!(
        infra.requests().find(&request_id).unwrap().unwrap().status,
        RequestStatus::Working
    );

    // Extend the seeded Containerfile beyond the base image.
    operator
        .workspace()
        .write_text(
            "Containerfile",
            &format!("FROM {BASE_REFERENCE}\nRUN echo building\n"),
        )
        .unwrap();
    let manifest_text = operator.workspace().read_text("manifest.toml").unwrap();
    assert!(
        manifest_text.contains(DEFAULT_TESTS_BLOCK),
        "seeded manifest must contain the default tests block:\n{manifest_text}"
    );
    json_result(
        &tools,
        "environment_manifest_update",
        json!({
            "environment_path": environment_path,
            "old_string": DEFAULT_TESTS_BLOCK,
            "new_string": "[[tests]]\nname = \"shell-present\"\nargv = [\"sh\", \"-eu\", \"-c\", \"command -v sh\"]",
        }),
    )
    .await;

    let validation = json_result(
        &tools,
        "environment_validate",
        json!({ "environment_path": environment_path }),
    )
    .await;
    assert_eq!(validation["passed"], json!(true), "{validation:?}");

    let build = json_result(
        &tools,
        "environment_build",
        json!({ "environment_path": environment_path }),
    )
    .await;
    assert_eq!(build["built"], json!(true), "{build:?}");
    let built_tag = build["local_tag"].as_str().unwrap().to_string();
    assert_eq!(
        built_tag,
        "localhost/auto-nomics/environments/bioconductor-extra:rsi-2"
    );

    let install = json_result(
        &tools,
        "environment_install",
        json!({ "environment_path": environment_path }),
    )
    .await;
    assert_eq!(install["activated"], json!(true), "{install:?}");
    let local_reference = install["catalog_reference"].as_str().unwrap().to_string();
    assert!(
        local_reference
            .starts_with("localhost/auto-nomics/environments/bioconductor-extra@sha256:"),
        "{local_reference}"
    );
    // Static validate (attempt 1) builds nothing; the build tool (attempt 2)
    // and install's revalidation (attempt 3) each exercise the builder.
    assert_eq!(harness.builder.build_count(), 2);
    // Debug runs target the last built image recorded in the manifest.
    let debug = json_result(
        &tools,
        "environment_container_run",
        json!({
            "environment_path": environment_path,
            "argv": ["sh", "-c", "echo marker"]
        }),
    )
    .await;
    assert_eq!(debug["exit_code"], json!(0));
    assert!(debug["stdout"].as_str().unwrap().contains("marker"));
    assert_eq!(
        harness
            .registry
            .get("bioconductor-extra")
            .unwrap()
            .unwrap()
            .reference,
        local_reference
    );
    assert_eq!(
        infra.requests().find(&request_id).unwrap().unwrap().status,
        RequestStatus::ReviewPending
    );

    let distiller = EnvironmentDistiller::new((*infra).clone());
    let report = distiller.run_once().await;
    assert_eq!(report.completed, 1, "{report:?}");
    let remote_reference =
        format!("ghcr.io/auto-nomics/environments/bioconductor-extra@{REMOTE_DIGEST}");
    assert_eq!(
        harness
            .registry
            .get("bioconductor-extra")
            .unwrap()
            .unwrap()
            .reference,
        remote_reference
    );
    let manifest = infra
        .store()
        .develop("bioconductor-extra")
        .unwrap()
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(manifest.status, EnvironmentStatus::Installed);
    assert_eq!(
        manifest.lifecycle.published_reference.as_deref(),
        Some(remote_reference.as_str())
    );
    assert_eq!(
        manifest.lifecycle.base_reference.as_deref(),
        Some(local_reference.as_str())
    );
    assert!(!manifest.lifecycle.publication_pending);
    assert_eq!(
        infra.requests().find(&request_id).unwrap().unwrap().status,
        RequestStatus::Consumed
    );

    let restored = infra.rollback_environment("bioconductor-extra").unwrap();
    assert_eq!(restored, local_reference);
    assert_eq!(
        harness
            .registry
            .get("bioconductor-extra")
            .unwrap()
            .unwrap()
            .reference,
        local_reference
    );

    infra.uninstall_environment("bioconductor-extra").unwrap();
    assert!(
        harness
            .registry
            .get("bioconductor-extra")
            .unwrap()
            .is_none()
    );
    assert!(
        harness
            .state
            .path()
            .join("environments/dev/bioconductor-extra/manifest.toml")
            .is_file(),
        "the development workspace must be retained"
    );
}

#[tokio::test]
async fn static_gates_reject_unsafe_containerfiles_and_secrets() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(true);
    let infra = harness.infra.clone();
    infra
        .create_environment(
            environment_request("demo-env", "Create demo environment"),
            "alpine",
        )
        .unwrap();

    let store = infra.store();
    let cases: &[(&str, &str)] = &[
        (
            "FROM docker.io/library/alpine:latest\n",
            "containerfile_static",
        ),
        (
            "FROM docker.io/library/debian@sha256:1111111111111111111111111111111111111111111111111111111111111111\nADD https://example.com/data.tsv /data.tsv\n",
            "containerfile_static",
        ),
        (
            "FROM docker.io/library/debian@sha256:1111111111111111111111111111111111111111111111111111111111111111\nRUN curl https://example.com/install.sh | sh\n",
            "containerfile_static",
        ),
        (
            "FROM docker.io/library/debian@sha256:1111111111111111111111111111111111111111111111111111111111111111\nCOPY assets/missing.txt /missing.txt\n",
            "containerfile_static",
        ),
        (
            "FROM docker.io/library/debian@sha256:1111111111111111111111111111111111111111111111111111111111111111\nRUN cp /mnt/host/data /data\n",
            "containerfile_static",
        ),
    ];
    for (containerfile, expected_gate) in cases {
        let operator = store.develop("demo-env").unwrap().unwrap();
        operator
            .workspace()
            .write_text("Containerfile", containerfile)
            .unwrap();
        let outcome = infra.validate_environment_local("demo-env").await.unwrap();
        let report = match outcome {
            plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => report,
            plugin_rsi::EnvironmentValidationOutcome::Passed(report) => {
                panic!("unsafe Containerfile passed: {containerfile} → {report:?}")
            }
        };
        assert!(
            report
                .gates
                .iter()
                .any(|gate| gate.name == *expected_gate && gate.status == GateStatus::Fail),
            "expected `{expected_gate}` to fail for {containerfile:?}: {report:?}"
        );
    }

    // A mutable manifest base reference fails the base_policy gate.
    let operator = store.develop("demo-env").unwrap().unwrap();
    {
        let text = operator.workspace().read_text("manifest.toml").unwrap();
        operator
            .workspace()
            .write_text(
                "manifest.toml",
                &text.replace(BASE_REFERENCE, "docker.io/library/alpine:latest"),
            )
            .unwrap();
    }
    let outcome = infra.validate_environment_local("demo-env").await.unwrap();
    let report = match outcome {
        plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => report,
        plugin_rsi::EnvironmentValidationOutcome::Passed(report) => {
            panic!("mutable base reference passed: {report:?}")
        }
    };
    assert!(
        report
            .gates
            .iter()
            .any(|gate| gate.name == "base_policy" && gate.status == GateStatus::Fail)
    );
    // Restore the pinned base for the remaining checks.
    let operator = store.develop("demo-env").unwrap().unwrap();
    {
        let text = operator.workspace().read_text("manifest.toml").unwrap();
        operator
            .workspace()
            .write_text(
                "manifest.toml",
                &text.replace("docker.io/library/alpine:latest", BASE_REFERENCE),
            )
            .unwrap();
    }

    // A credential anywhere in the workspace blocks the build gates.
    let operator = store.develop("demo-env").unwrap().unwrap();
    operator
        .workspace()
        .write_text(
            "Containerfile",
            &format!("FROM {BASE_REFERENCE}\nRUN echo ok\n"),
        )
        .unwrap();
    operator
        .workspace()
        .write_text("notes.txt", "token ghp_0123456789abcdef\n")
        .unwrap();
    let outcome = infra.validate_environment_local("demo-env").await.unwrap();
    let report = match outcome {
        plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => report,
        plugin_rsi::EnvironmentValidationOutcome::Passed(report) => {
            panic!("secret scan passed: {report:?}")
        }
    };
    assert!(
        report
            .gates
            .iter()
            .any(|gate| gate.name == "secret_scan" && gate.status == GateStatus::Fail)
    );
}

#[tokio::test]
async fn missing_builder_blocks_infrastructure_gates() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(false);
    let infra = harness.infra.clone();
    infra
        .create_environment(
            environment_request("blocked-env", "Create blocked environment"),
            "alpine",
        )
        .unwrap();

    let outcome = infra.build_environment("blocked-env").await.unwrap();
    let report = match outcome {
        plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => report,
        plugin_rsi::EnvironmentValidationOutcome::Passed(report) => {
            panic!("missing infrastructure passed validation: {report:?}")
        }
    };
    assert_eq!(report.overall, GateStatus::Blocked);
    let build_gate = report
        .gates
        .iter()
        .find(|gate| gate.name == "build")
        .expect("build gate present");
    assert_eq!(build_gate.status, GateStatus::Blocked);
    assert_eq!(
        infra
            .store()
            .develop("blocked-env")
            .unwrap()
            .unwrap()
            .status(),
        EnvironmentStatus::NeedsFix
    );
}

#[tokio::test]
async fn failing_smoke_tests_repair_the_workspace() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(true);
    let infra = harness.infra.clone();
    let tools = plugin_rsi::environment_development_tool_registrations("env-smoke-agent");
    infra
        .create_environment(
            environment_request("smoke-env", "Create smoke environment"),
            "alpine",
        )
        .unwrap();
    json_result(
        &tools,
        "environment_manifest_update",
        json!({
            "environment_path": "/environments/dev/smoke-env",
            "old_string": DEFAULT_TESTS_BLOCK,
            "new_string": "[[tests]]\nname = \"explodes\"\nargv = [\"boom\"]",
        }),
    )
    .await;

    let outcome = infra.build_environment("smoke-env").await.unwrap();
    let report = match outcome {
        plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => report,
        plugin_rsi::EnvironmentValidationOutcome::Passed(report) => {
            panic!("failing smoke test passed validation: {report:?}")
        }
    };
    assert!(
        report
            .gates
            .iter()
            .any(|gate| gate.name == "smoke" && gate.status == GateStatus::Fail)
    );
    assert_eq!(
        infra
            .store()
            .develop("smoke-env")
            .unwrap()
            .unwrap()
            .status(),
        EnvironmentStatus::NeedsFix
    );
}

/// A valid edit lands (and resets a pending local activation); every rejected
/// edit — not-found, invalid TOML, host-owned field tampering, digest-unpinned
/// base, malformed interpreters — leaves manifest.toml byte-identical.
#[tokio::test]
async fn manifest_update_edits_are_validated_before_they_land() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(false);
    let infra = harness.infra.clone();
    let tools = plugin_rsi::environment_development_tool_registrations("env-manifest-agent");
    let environment_path = "/environments/dev/manifest-env";
    infra
        .create_environment(
            environment_request("manifest-env", "Create manifest environment"),
            "alpine",
        )
        .unwrap();
    let store = infra.store();
    let operator = store.develop("manifest-env").unwrap().unwrap();
    let read_manifest = || operator.workspace().read_text("manifest.toml").unwrap();

    // Simulate a pending local activation; any successful edit must reset it.
    let pending =
        read_manifest().replace("publication_pending = false", "publication_pending = true");
    assert_ne!(pending, read_manifest());
    operator
        .workspace()
        .write_text("manifest.toml", &pending)
        .unwrap();

    // Successful edit: swap the seeded smoke test for a real one.
    let value = json_result(
        &tools,
        "environment_manifest_update",
        json!({
            "environment_path": environment_path,
            "old_string": DEFAULT_TESTS_BLOCK,
            "new_string": "[[tests]]\nname = \"shell-present\"\nargv = [\"sh\", \"-eu\", \"-c\", \"command -v sh\"]",
        }),
    )
    .await;
    assert_eq!(value["test_count"], json!(1), "{value:?}");
    let updated = read_manifest();
    assert!(
        updated.contains("name = \"shell-present\""),
        "edit must land on disk:\n{updated}"
    );
    assert!(
        updated.contains("publication_pending = false"),
        "a manifest edit must invalidate pending local activation:\n{updated}"
    );

    let original = read_manifest();
    let base_line = format!("reference = \"{BASE_REFERENCE}\"");
    let rejects: Vec<(&str, Value)> = vec![
        (
            "not found",
            json!({
                "environment_path": environment_path,
                "old_string": "NONSENSE-NOT-IN-FILE",
                "new_string": "x",
            }),
        ),
        (
            "invalid toml",
            json!({
                "environment_path": environment_path,
                "old_string": "argv = [\"sh\", \"-eu\", \"-c\", \"command -v sh\"]",
                "new_string": "argv = [",
            }),
        ),
        (
            "status tamper",
            json!({
                "environment_path": environment_path,
                "old_string": "status = \"draft\"",
                "new_string": "status = \"approved\"",
            }),
        ),
        (
            "lifecycle tamper",
            json!({
                "environment_path": environment_path,
                "old_string": "rationale = \"Create manifest environment\"",
                "new_string": "rationale = \"tampered\"",
            }),
        ),
        (
            "identity tamper",
            json!({
                "environment_path": environment_path,
                "old_string": "environment_id = \"manifest-env\"",
                "new_string": "environment_id = \"other-env\"",
            }),
        ),
        (
            "unpinned base",
            json!({
                "environment_path": environment_path,
                "old_string": base_line,
                "new_string": "reference = \"docker.io/library/alpine:latest\"",
            }),
        ),
        (
            "duplicate interpreters",
            json!({
                "environment_path": environment_path,
                "old_string": "interpreters = [\"sh\"]",
                "new_string": "interpreters = [\"sh\", \"SH\"]",
            }),
        ),
    ];
    for (hint, input) in rejects {
        let error = execute(&tools, "environment_manifest_update", input)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ToolError::ValidationFailed { .. }),
            "{hint} must be a validation rejection: {error:?}"
        );
        assert_eq!(
            read_manifest(),
            original,
            "{hint} rejection must leave manifest.toml untouched"
        );
    }
}

/// Repeated identical blocks are ambiguous without `replace_all` and
/// replace-everywhere with it.
#[tokio::test]
async fn manifest_update_requires_disambiguation_for_repeated_blocks() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(false);
    let infra = harness.infra.clone();
    let tools = plugin_rsi::environment_development_tool_registrations("env-twin-agent");
    infra
        .create_environment(
            environment_request("twin-env", "Create twin environment"),
            "alpine",
        )
        .unwrap();
    let store = infra.store();
    let operator = store.develop("twin-env").unwrap().unwrap();

    const TWIN_BLOCK: &str = "[[tests]]\nname = \"twin\"\nargv = [\"true\"]";
    let text = operator.workspace().read_text("manifest.toml").unwrap();
    let doubled = text.replace(
        DEFAULT_TESTS_BLOCK,
        &format!("{TWIN_BLOCK}\n\n{TWIN_BLOCK}"),
    );
    assert_ne!(
        doubled, text,
        "seeded manifest must contain the default block"
    );
    operator
        .workspace()
        .write_text("manifest.toml", &doubled)
        .unwrap();

    let error = execute(
        &tools,
        "environment_manifest_update",
        json!({
            "environment_path": "/environments/dev/twin-env",
            "old_string": TWIN_BLOCK,
            "new_string": "[[tests]]\nname = \"twin2\"\nargv = [\"true\"]",
        }),
    )
    .await
    .unwrap_err();
    match error {
        ToolError::ValidationFailed { message } => {
            assert!(
                message.contains('2'),
                "ambiguity must report the match count: {message}"
            );
        }
        other => panic!("repeated block must be ambiguous: {other:?}"),
    }
    assert_eq!(
        operator.workspace().read_text("manifest.toml").unwrap(),
        doubled,
        "ambiguous edit must not land"
    );

    let value = json_result(
        &tools,
        "environment_manifest_update",
        json!({
            "environment_path": "/environments/dev/twin-env",
            "old_string": TWIN_BLOCK,
            "new_string": "[[tests]]\nname = \"twin2\"\nargv = [\"true\"]",
            "replace_all": true,
        }),
    )
    .await;
    assert_eq!(value["test_count"], json!(2), "{value:?}");
    // `operator` caches the manifest from develop time; re-develop to see
    // what the tool landed on disk.
    let operator = store.develop("twin-env").unwrap().unwrap();
    assert!(
        operator
            .manifest()
            .tests
            .iter()
            .all(|test| test.name == "twin2"),
        "replace_all must rewrite every repeated block: {:?}",
        operator.manifest().tests
    );
}

#[tokio::test]
async fn dirty_workspaces_refuse_local_activation() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(true);
    let infra = harness.infra.clone();
    infra
        .create_environment(
            environment_request("dirty-env", "Create dirty environment"),
            "alpine",
        )
        .unwrap();
    let store = infra.store();
    let operator = store.develop("dirty-env").unwrap().unwrap();
    operator
        .workspace()
        .write_text(
            "Containerfile",
            &format!("FROM {BASE_REFERENCE}\nRUN echo ok\n"),
        )
        .unwrap();

    match infra.build_environment("dirty-env").await.unwrap() {
        plugin_rsi::EnvironmentValidationOutcome::Passed(_) => {}
        plugin_rsi::EnvironmentValidationOutcome::NeedsFix(report) => {
            panic!("clean build failed: {report:?}")
        }
    }
    operator
        .workspace()
        .write_text("stray.txt", "uncommitted\n")
        .unwrap();
    let error = infra
        .store()
        .install_local("dirty-env", &harness.registry)
        .unwrap_err();
    assert!(
        error.to_string().contains("uncommitted"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn lifecycle_mutations_are_guarded_by_transitions() {
    let _guard = SEQUENTIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let harness = harness(true);
    let infra = harness.infra.clone();
    infra
        .create_environment(
            environment_request("guarded-env", "Create guarded environment"),
            "alpine",
        )
        .unwrap();

    // A draft environment cannot be reviewed directly.
    let error = infra.review_environment("guarded-env", true).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid plugin transition draft -> approved"),
        "unexpected error: {error}"
    );

    // Forking keeps content but resets lifecycle facts.
    let operator = infra
        .fork_environment(
            "guarded-env",
            environment_request("guarded-fork", "Fork the guarded environment"),
        )
        .unwrap();
    let manifest = operator.manifest();
    assert_eq!(manifest.environment_id, "guarded-fork");
    assert_eq!(manifest.status, EnvironmentStatus::Draft);
    assert_eq!(
        manifest.lifecycle.source_environment.as_deref(),
        Some("guarded-env")
    );
    assert!(operator.workspace().read_text("Containerfile").is_ok());
}
