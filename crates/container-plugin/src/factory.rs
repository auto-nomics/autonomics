//! The bridge between one manifest node definition and the dag-core
//! registry: [`ManifestNodeFactory`] implements [`NodeFactory`] by
//! compiling the declarative [`NodeDefinition`] through the M2 compilers
//! and delegating execution to [`ContainerCommandNode`].
//!
//! One factory serves one `[[nodes]]` entry. A family's [`Plugin`]
//! registers one factory per entry.
//!
//! What this layer deliberately does **not** do: parse TOML (the loader's
//! job — factories receive already-parsed structs), re-validate manifest
//! structure (load-time, before any factory exists), or execute anything
//! (the node's job). It is the thinnest possible adapter.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::error::Result as RegistryResult;
use dag_core::registry::{NodeCtx, NodeFactory, NodeRegistry};
use dag_core::{DataBundle, DataBundleBinding, NodePlugin, NodePorts};
use nodes_io::container_command::ContainerCommandNode;

use crate::compile::compile_schema;
use crate::compile::spec_compile::compile_container_spec;
use crate::manifest::{ImageMetadata, PanelBinding, PluginManifest};
use crate::node_definition::{NodeDefinition, compile_ports};

/// A [`NodeFactory`] backed by one manifest node definition.
///
/// The `&'static str` trio (`kind`, `desc`, `doc`) is leaked once at
/// construction: the trait requires `'static`, manifests are runtime
/// data, and the count is bounded by the node count for the process
/// lifetime.
#[derive(Clone)]
pub struct ManifestNodeFactory {
    entry: NodeDefinition,
    image: ImageMetadata,
    panels: Vec<PanelBinding>,
    runtime: Arc<dyn container_runtime::PodmanConnection>,
    panel_cache: Arc<container_runtime::PanelCache>,
    kind: &'static str,
    desc: &'static str,
    doc: &'static str,
    ports: NodePorts,
}

impl ManifestNodeFactory {
    /// Construct a factory from one node definition plus its family-level
    /// image and panel bindings. Callers are expected to have run
    /// [`crate::node_definition::validate`] already; this constructor
    /// trusts the definition and leaks its identity strings.
    pub fn new(
        entry: NodeDefinition,
        image: ImageMetadata,
        panels: Vec<PanelBinding>,
        runtime: Arc<dyn container_runtime::PodmanConnection>,
        panel_cache: Arc<container_runtime::PanelCache>,
    ) -> Self {
        let ports = compile_ports(&entry.ports);
        Self {
            kind: Box::leak(entry.kind.clone().into_boxed_str()),
            desc: Box::leak(entry.desc.clone().into_boxed_str()),
            doc: Box::leak(entry.doc.clone().into_boxed_str()),
            ports,
            entry,
            image,
            panels,
            runtime,
            panel_cache,
        }
    }

    fn panel_bindings(&self) -> Vec<DataBundleBinding> {
        self.panels
            .iter()
            .map(|panel| DataBundleBinding::new(panel.binding.clone(), panel.bundle.to_string()))
            .collect()
    }
}

impl NodeFactory for ManifestNodeFactory {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn desc(&self) -> &'static str {
        self.desc
    }

    fn doc(&self) -> &'static str {
        self.doc
    }

    fn deprecated(&self) -> bool {
        self.entry.deprecated
    }

    fn spec_schema(&self) -> schemars::Schema {
        compile_schema(&self.entry.params)
    }

    fn ports(&self) -> NodePorts {
        self.ports.clone()
    }

    fn ports_for_spec(&self, _spec: serde_json::Value) -> RegistryResult<NodePorts> {
        // v0 has no dynamic ports (conditional/variadic inputs arrive with
        // the lava_scan migration wave); the static layout is the layout.
        Ok(self.ports())
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        self.panel_bindings()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> RegistryResult<Vec<DataBundleBinding>> {
        // v0 panels are static; `from_param` selection arrives later.
        Ok(self.data_bundles())
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> RegistryResult<Box<dyn DagNode>> {
        // `spec` arrives post-normalization: NodeRegistry::build_node has
        // already repaired common LLM pathologies against our compiled
        // schema. Everything below is strict.
        let container_spec = compile_container_spec(&self.entry, &self.image, &self.panels, &spec)
            .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;

        let bundles = self
            .panels
            .iter()
            .map(|panel| node_ctx.bound_data_bundle(&panel.binding).cloned())
            .collect::<RegistryResult<Vec<DataBundle>>>()?;

        let node = ContainerCommandNode::new_with_catalog_panels(
            self.kind,
            container_spec,
            self.runtime.clone(),
            self.panel_cache.clone(),
            bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;

        Ok(Box::new(node))
    }
}

/// A manifest family exposed as a [`NodePlugin`]: registers one
/// [`ManifestNodeFactory`] per `[[nodes]]` entry.
pub struct Plugin {
    name: &'static str,
    panels: Vec<PanelBinding>,
    factories: Vec<ManifestNodeFactory>,
}

impl Plugin {
    /// Build the plugin from a parsed family manifest plus the shared
    /// execution infrastructure injected by the runtime host.
    pub fn new(
        manifest: PluginManifest,
        runtime: Arc<dyn container_runtime::PodmanConnection>,
        panel_cache: Arc<container_runtime::PanelCache>,
    ) -> Self {
        let name: &'static str = Box::leak(manifest.plugin_name.clone().into_boxed_str());
        let panels = manifest.panels.clone();
        let factories = manifest
            .nodes
            .into_iter()
            .map(|entry| {
                ManifestNodeFactory::new(
                    entry,
                    manifest.image.clone(),
                    manifest.panels.clone(),
                    runtime.clone(),
                    panel_cache.clone(),
                )
            })
            .collect();
        Self {
            name,
            panels,
            factories,
        }
    }
}

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        self.name
    }

    fn register(&self, registry: &mut NodeRegistry) {
        for factory in &self.factories {
            registry.register(Box::new(factory.clone()));
        }
    }
}

impl Plugin {
    /// The node kinds this family contributes; used by the loader for
    /// cross-family duplicate detection.
    pub fn registered_kinds(&self) -> Vec<&'static str> {
        self.factories
            .iter()
            .map(|factory| factory.kind())
            .collect()
    }

    /// The family-level `[[panels]]` bindings. Every node in the family
    /// mounts all of these; the startup preflight uses the list to
    /// provision the referenced catalog bundles.
    pub fn panels(&self) -> &[PanelBinding] {
        &self.panels
    }
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Factories carry runtime handles and full definitions; summarize
        // instead of dumping them.
        f.debug_struct("Plugin")
            .field("name", &self.name)
            .field("kinds", &self.registered_kinds())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use dag_core::registry::NodeRegistry;
    use dag_core::value::{FileRef, NodeValue, PortType};
    use dag_core::{NodeCtx, NodeInput};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use container_runtime::{
        ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, DEFAULT_CONTAINER_WORKDIR,
        PanelCache, PodmanConnection,
    };

    /// Minimal fake: records every request, copies the first input to the
    /// first declared output (the pass-through contract used by the
    /// container_command tests).
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
            // Write every declared output so the node's post-run check
            // (each declared output must exist) passes for multi-output
            // manifests.
            for (key, value) in &request.env {
                // Match AUTONOMICS_OUTPUT<digits> only: AUTONOMICS_OUTPUT_COUNT
                // shares the prefix but carries a count, not a /work path.
                let Some(index) = key.strip_prefix("AUTONOMICS_OUTPUT") else {
                    continue;
                };
                if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let relative = Path::new(value)
                    .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
                    .expect("output is inside /work");
                let host_output = request.workspace.host_path.join(relative);
                std::fs::write(&host_output, "plugin-factory-result").unwrap();
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

    const LDSC_MANIFEST_TOML: &str = r#"
schema_version = 1
plugin_name = "ldsc"

[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"

[[panels]]
binding = "ref_ld"
mount = "/panels/ref_ld"
bundle = "wjixiang/catalog-ldsc-ref-ld-1000g-eur-basic"

[[panels]]
binding = "w_ld"
mount = "/panels/w_ld"
bundle = "wjixiang/catalog-ldsc-w-ld-1000g-eur-hm3-no-mhc"

[[nodes]]
kind = "ldsc_h2"
desc = "Runs official LDSC 3.0.1 h2 on one GWAS sumstats file."
doc = "Estimates SNP heritability with the official LDSC continuation. The input is one tab-separated sumstats File with SNP, A1, A2, N, and Z columns; plain .tsv and gzip .sumstats.gz are both accepted."
timeout_secs = 3600

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [
    { path = "ldsc_h2.log", format = "ldsc_log" },
]

[nodes.params]
intercept = { type = "number", default = 1.0, doc = "Liability-scale intercept" }

[nodes.command]
interpreter = "sh"
script = """
set -eu
ldsc --h2 "$AUTONOMICS_INPUT0" \
  --ref-ld-chr /panels/ref_ld/LDscore. \
  --w-ld-chr /panels/w_ld/weights.hm3_noMHC. \
  --intercept {{ intercept }} \
  --out "$AUTONOMICS_WORKDIR/ldsc_h2" > "$AUTONOMICS_OUTPUT0" 2>&1
"""
"#;

    fn test_manifest() -> PluginManifest {
        toml::from_str(LDSC_MANIFEST_TOML).expect("fixture manifest parses")
    }

    fn registry_with_plugin(runtime: Arc<FakeRuntime>) -> NodeRegistry {
        let workspace = tempfile::tempdir().unwrap();
        let panel_cache = Arc::new(PanelCache::new(workspace.path().join("panels")));
        let plugin = Plugin::new(
            test_manifest(),
            runtime as Arc<dyn PodmanConnection>,
            panel_cache,
        );
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut registry = NodeRegistry::new(ctx);
        registry.register_plugin(&plugin);
        registry
    }

    #[test]
    fn plugin_registers_the_manifest_node_kind() {
        let runtime = Arc::new(FakeRuntime {
            workspace_root: PathBuf::from("/tmp"),
            requests: Mutex::new(Vec::new()),
        });
        let panel_cache = Arc::new(PanelCache::new(PathBuf::from("/tmp/plugin-test-panels")));
        let plugin = Plugin::new(
            test_manifest(),
            runtime as Arc<dyn PodmanConnection>,
            panel_cache,
        );
        assert_eq!(plugin.name(), "ldsc", "family name comes from the manifest");

        let mut registry = NodeRegistry::new(NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        ));
        registry.register_plugin(&plugin);
        assert!(
            registry
                .list_nodes()
                .iter()
                .any(|node| node.kind == "ldsc_h2"),
            "the manifest kind must appear in the registry"
        );
    }

    #[test]
    fn spec_schema_is_the_compiled_params_object() {
        let runtime = Arc::new(FakeRuntime {
            workspace_root: PathBuf::from("/tmp"),
            requests: Mutex::new(Vec::new()),
        });
        let registry = registry_with_plugin(runtime);
        let schema = serde_json::to_value(registry.get_node_spec("ldsc_h2").unwrap()).unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"]["intercept"]["type"] == "number");
    }

    /// Restores AUTONOMICS_KEEP_WORKSPACE on drop so one test's debugging
    /// preference cannot leak into other tests.
    struct EnvReset(&'static str);
    impl Drop for EnvReset {
        fn drop(&mut self) {
            // SAFETY: process-global env mutation, dropped at test end;
            // no other test in this module reads this variable.
            unsafe { std::env::remove_var(self.0) };
        }
    }

    #[tokio::test]
    async fn build_and_execute_end_to_end_through_the_fake_runtime() {
        // Keep the scratch directory so the staged script survives the
        // successful-run cleanup and can be inspected below.
        let _reset = EnvReset(container_runtime::KEEP_WORKSPACE_ENV);
        // SAFETY: guarded by `_reset`; restored on drop.
        unsafe { std::env::set_var(container_runtime::KEEP_WORKSPACE_ENV, "1") };
        // The M3 acceptance gate: manifest -> registry -> build_node ->
        // execute -> FakeRuntime receives a request whose argv/env/script
        // match the compiled contract, and the output publishes to VFS.
        let workspace = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        let storage = Arc::new(vfs::OpendalFileStorage::new(objects.path()));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage),
        );

        let runtime = Arc::new(FakeRuntime {
            workspace_root: workspace.path().to_path_buf(),
            requests: Mutex::new(Vec::new()),
        });
        let panel_cache = Arc::new(PanelCache::new(workspace.path().join("panels")));
        // The e2e run uses a panel-free variant: PanelCache would try to
        // materialize and checksum the referenced bundle, which is the
        // loader/runtime's concern, not the factory's. The registry test
        // above already covers the panel binding wiring (both LDSC panels).
        let mut manifest = test_manifest();
        manifest.panels.clear();
        let plugin = Plugin::new(
            manifest,
            runtime.clone() as Arc<dyn PodmanConnection>,
            panel_cache,
        );
        let mut registry = NodeRegistry::new(ctx.clone());
        registry.register_plugin(&plugin);

        let input = workspace.path().join("trait1.tsv");
        std::fs::write(&input, "snpid\tz\nrs1\t2.0\n").unwrap();
        let input_ref = FileRef::local(&input, Some("mtag_sumstats".into())).unwrap();

        let mut node = registry
            .build_node("ldsc_h2", serde_json::json!({"intercept": 2.0}))
            .expect("manifest node builds");

        let outputs = node
            .execute(
                &ctx,
                &[NodeInput {
                    port: 0,
                    data: NodeValue::File(input_ref),
                }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("manifest node executes");

        let request = runtime.requests.lock().unwrap().last().unwrap().clone();
        // Image and kind are the manifest's, the renderer's output is in
        // the staged script, and both declared outputs were produced.
        assert!(
            request
                .image
                .starts_with("ghcr.io/auto-nomics/autonomics/ldsc@sha256:")
        );
        let script =
            std::fs::read_to_string(request.workspace.host_path.join(".autonomics/script"))
                .unwrap();
        assert!(
            script.contains("--intercept 2.0"),
            "renderer output must appear in the staged script: {script}"
        );
        assert_eq!(outputs.iter().count(), 1);
    }
}
