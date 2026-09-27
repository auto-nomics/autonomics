//! End-to-end ldsc plugin verification: git source from `plugins.toml`
//! -> sync (clone at pinned rev) -> loader -> registry -> build_node ->
//! execute against a hermetic fake runtime.
//!
//! Requires network and the pushed plugin repository; run explicitly:
//! `cargo test -p container-plugin --test ldsc_e2e -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dag_core::registry::NodeRegistry;
use dag_core::value::{FileRef, NodeValue};
use dag_core::{NodeCtx, NodeInput, NodePlugin};
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache,
    PodmanConnection, DEFAULT_CONTAINER_WORKDIR,
};

struct FakeRuntime {
    workspace_root: PathBuf,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

#[async_trait]
impl PodmanConnection for FakeRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        for (key, value) in &request.env {
            let Some(index) = key.strip_prefix("AUTONOMICS_OUTPUT") else {
                continue;
            };
            if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let relative = Path::new(value)
                .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
                .expect("output is inside /work");
            std::fs::write(request.workspace.host_path.join(relative), "e2e-result").unwrap();
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake"
    }

    fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

/// Restores AUTONOMICS_KEEP_WORKSPACE on drop so this test's debugging
/// preference cannot leak.
struct EnvReset(&'static str);
impl Drop for EnvReset {
    fn drop(&mut self) {
        // SAFETY: single test, no other reader in this binary.
        unsafe { std::env::remove_var(self.0) };
    }
}

#[tokio::test]
#[ignore = "requires network access to clone the pushed plugin repository"]
async fn ldsc_plugin_installs_from_git_and_executes() {
    // Keep the scratch directory so the staged script survives the
    // successful-run cleanup and can be inspected below.
    let _reset = EnvReset(container_runtime::KEEP_WORKSPACE_ENV);
    // SAFETY: guarded by `_reset`; restored on drop.
    unsafe { std::env::set_var(container_runtime::KEEP_WORKSPACE_ENV, "1") };
    let state = tempfile::tempdir().unwrap();
    let config_path = state.path().join("plugins.toml");
    std::fs::write(
        &config_path,
        r#"
[[plugin]]
name = "ldsc"
git = "git@github.com:auto-nomics/ldsc-plugin.git"
rev = "15264fa24a273e2655ba2f887949ff5a9bdeee0d"
"#,
    )
    .unwrap();
    let root = state.path().join("plugins");

    // 1. Install: sync clones the pinned commit.
    let report = container_plugin::sync::sync(&config_path, &root).unwrap();
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].0, "ldsc");
    assert!(root.join("ldsc/manifest.toml").is_file(), "plugin tree cloned");
    assert!(root.join("ldsc/scripts/h2.sh").is_file(), "script present");

    // 2. Load with a hermetic fake runtime; register through the real
    //    registry path with catalog-backed panel stubs.
    let fake = Arc::new(FakeRuntime {
        workspace_root: root.join("workspace"),
        requests: Mutex::new(Vec::new()),
    });
    let plugins = container_plugin::loader::load(
        &root,
        fake.clone() as Arc<dyn PodmanConnection>,
        Arc::new(PanelCache::new(root.join("panels"))),
    )
    .unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name(), "ldsc");

    // PanelCache verifies every payload checksum, so the stub bundles need
    // real manifests and payloads in the fake object store.
    fn materialize_panel(objects_root: &Path, id: &str, payload: &[u8]) -> dag_core::DataBundle {
        use sha2::{Digest, Sha256};
        let entry = objects_root.join(format!("datasets/{id}@sha256:e2e"));
        std::fs::create_dir_all(entry.join("chr22")).unwrap();
        std::fs::write(entry.join("chr22/panel.txt"), payload).unwrap();
        std::fs::write(
            entry.join("manifest.json"),
            serde_json::to_vec(&container_runtime::PanelManifest {
                schema_version: 1,
                id: id.into(),
                version: "e2e".into(),
                digest: format!("sha256:{}", "1".repeat(64)),
                files: vec![container_runtime::PanelFile {
                    path: "chr22/panel.txt".into(),
                    size: payload.len() as u64,
                    sha256: format!("sha256:{}", Sha256::digest(payload).iter().map(|b| format!("{b:02x}")).collect::<String>()),
                }],
            })
            .unwrap(),
        )
        .unwrap();
        let mut bundle = dag_core::DataBundle::new(id, "panel", format!("/bundles/{id}"));
        bundle.source = Some(format!("/datasets/{id}@sha256:e2e"));
        bundle.digest = Some(format!("sha256:{}", "1".repeat(64)));
        bundle
    }

    let objects = tempfile::tempdir().unwrap();
    let storage = Arc::new(vfs::OpendalFileStorage::new(objects.path()));
    let mut bundles = dag_core::BundleRegistry::new();
    let ref_ld = materialize_panel(objects.path(), "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic", b"ref-ld-scores");
    let w_ld = materialize_panel(objects.path(), "wjixiang/catalog-ldsc-w-ld-1000g-eur-hm3-no-mhc", b"w-ld-weight".as_slice());
    bundles.register(ref_ld).unwrap();
    bundles.register(w_ld).unwrap();
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    )
    .with_bundle_registry(Arc::new(bundles));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register_plugin(&plugins[0]);

    // 3. Build and execute: submitted params, hermetic run.
    let input = state.path().join("sumstats.tsv");
    std::fs::write(&input, "SNP\tA1\tA2\tN\tZ\nrs1\tA\tG\t1000\t2.0\n").unwrap();
    let mut node = registry
        .build_node(
            "ldsc_h2_container",
            serde_json::json!({"n_blocks": 150, "intercept_h2": 1.1}),
        )
        .expect("plugin node builds");

    let outputs = node
        .execute(
            &ctx,
            &[NodeInput {
                port: 0,
                data: NodeValue::File(FileRef::local(&input, Some("sumstats".into())).unwrap()),
            }],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("plugin node executes");

    // 4. Verify the executed contract.
    let requests = fake.requests.lock().unwrap();
    let request = requests.last().unwrap();
    assert!(
        request
            .image
            .starts_with("ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9"),
        "pinned image: {}",
        request.image
    );
    assert_eq!(request.command.first().unwrap(), "sh");
    let script =
        std::fs::read_to_string(request.workspace.host_path.join(".autonomics/script")).unwrap();
    assert!(script.contains("--n-blocks \"$LDSC_N_BLOCKS\""), "{script}");
    let env: BTreeMap<_, _> = request.env.iter().cloned().collect();
    assert_eq!(env.get("LDSC_N_BLOCKS").unwrap(), "150");
    assert_eq!(env.get("LDSC_INTERCEPT_H2").unwrap(), "1.1");
    assert_eq!(env.get("LDSC_CHISQ_MAX").unwrap(), "");
    assert_eq!(outputs.iter().count(), 1);
}
