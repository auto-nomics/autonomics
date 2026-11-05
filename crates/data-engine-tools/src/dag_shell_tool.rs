use crate::ExecError;
use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;
use std::sync::Arc;

#[tool(
    name = "dag_shell",
    description = "Edit the session flow transactionally with sandboxed Python syntax. Use from_path(id, pattern_or_list, hidden=False, checkIfExists=False, glob=True, maxDepth=None, type='file'), from_file_pairs(id, pattern_or_list, size=None, flat=False, hidden=False, checkIfExists=False, glob=True, maxDepth=None, type='file', grouping=None), from_list(id, values), channel_value(id, value), process(id, kind, spec, inputs=[channel, ...]), output(id) for an existing process or source, bind_inputs(id, [channel, ...]) to replace inputs, update_node, remove_node, and commit(); process returns an output channel or named outputs, and input bindings create connections automatically. Loops and conditionals are supported. Changes are validated and installed atomically; dry_run validates without changing the flow. Queries inspect the flow as it existed when the script started."
)]
pub struct DagShellInput {
    pub script: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}
pub struct DagShellTool {
    client: Arc<DataEngineClient>,
}
impl DagShellTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}
#[async_trait]
impl ToolFunction for DagShellTool {
    type Input = DagShellInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let result = self
            .client
            .run_dag_shell(input.script, input.dry_run, input.timeout_ms)
            .await
            .map_err(ExecError::from)?;
        let value = serde_json::to_value(result).map_err(|e| ToolError::ValidationFailed {
            message: e.to_string(),
        })?;
        Ok(ToolResult::success_json(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_engine::data_engine::DataEngine;

    #[tokio::test]
    async fn shell_tuple_channel_runs_registered_container_plugin() {
        use container_runtime::{
            ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PodmanConnection,
        };
        struct Runtime {
            root: std::path::PathBuf,
            seen: std::sync::Mutex<Vec<(i64, Vec<u8>)>>,
        }
        #[async_trait::async_trait]
        impl PodmanConnection for Runtime {
            async fn run(
                &self,
                request: ContainerRunRequest,
            ) -> Result<ContainerRunResult, ContainerRuntimeError> {
                let get = |name: &str| {
                    request
                        .env
                        .iter()
                        .find(|(key, _)| key == name)
                        .unwrap()
                        .1
                        .clone()
                };
                let sample: i64 =
                    serde_json::from_str(&get("AUTONOMICS_TUPLE_JSON_sample")).unwrap();
                let path: String =
                    serde_json::from_str(&get("AUTONOMICS_TUPLE_JSON_reads")).unwrap();
                let host_path = request
                    .workspace
                    .host_path
                    .join(std::path::Path::new(&path).strip_prefix("/work").unwrap());
                self.seen
                    .lock()
                    .unwrap()
                    .push((sample, std::fs::read(host_path).unwrap()));
                let output = get("AUTONOMICS_OUTPUT0");
                let output_path = request
                    .workspace
                    .host_path
                    .join(std::path::Path::new(&output).strip_prefix("/work").unwrap());
                std::fs::write(output_path, b"ok").unwrap();
                std::fs::write(request.workspace.host_path.join("b.dat"), b"second").unwrap();
                std::fs::write(request.workspace.host_path.join("a.dat"), b"first").unwrap();
                std::fs::write(request.workspace.host_path.join(".secret.dat"), b"hidden").unwrap();
                Ok(ContainerRunResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            fn name(&self) -> &'static str {
                "tuple-test"
            }
            fn workspace_root(&self) -> &std::path::Path {
                &self.root
            }
        }
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("plugins");
        let plugin = plugins.join("tuple_test");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(plugin.join("manifest.toml"), r#"
schema_version = 1
plugin_name = "tuple_test"
[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"
[[nodes]]
kind = "tuple_test_node"
desc = "test tuple"
doc = "test tuple"
[nodes.ports]
inputs = [{type="tuple", tuple=[{val="sample"},{path="reads", arity="1"}]}]
outputs = [{path="out.txt", tuple=[{val="sample"},{path="out.txt"},{path="*.dat", arity="2", hidden=false}]}, {path="missing.txt", optional=true, tuple=[{val="sample"},{path="missing.txt"}]}]
[nodes.command]
interpreter = "sh"
script = "cat \"$reads\" > /work/out.txt"
"#).unwrap();
        let a = root.path().join("a.fastq");
        let b = root.path().join("b.fastq");
        std::fs::write(&a, b"reads-a").unwrap();
        std::fs::write(&b, b"reads-b").unwrap();
        let runtime = Arc::new(Runtime {
            root: root.path().to_path_buf(),
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let config = container_runtime::PodmanConfig::for_state_dir(root.path());
        let infra = Arc::new(container_runtime::ContainerExecutionInfra {
            runtime: runtime.clone(),
            panel_cache: Arc::new(container_runtime::PanelCache::new(
                root.path().join("panels"),
            )),
            config,
        });
        let engine = DataEngine::builder()
            .with_plugins_root(plugins.clone())
            .with_container_execution(infra.clone())
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let items = serde_json::json!([[1, a.to_str().unwrap()], [2, b.to_str().unwrap()]]);
        let script = format!(
            "def correct(item):\n    return (item[0] == 1 or item[0] == 2) and len(item[2]) == 2 and item[2][0].name == 'a.dat' and item[2][1].name == 'b.dat'\ndef output_file(item):\n    return item[2][0]\nfrom_list('reads', {items})\nnode('qc', 'tuple_test/tuple_test_node', {{}})\nnode('check', 'channel', {{'operator':'filter', 'function':correct}})\nnode('file', 'channel', {{'operator':'map', 'function':output_file}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nnode('absent', 'channel', {{'operator':'filter', 'function':correct}})\nnode('absentfile', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('absent', 0, 'absentfile', 0)\nedge('reads', 0, 'qc', 0)\nedge('qc', 0, 'check', 0)\nedge('qc', 1, 'absent', 0)\nedge('check', 0, 'file', 0)\nedge('file', 0, 'read', 0)\ncommit()\n"
        );
        let edit = client
            .run_dag_shell(script.clone(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["qc"].succeeded, 2);
        assert_eq!(report.processes["read"].succeeded, 2);
        assert_eq!(report.processes["absentfile"].succeeded, 0);
        let mut seen = runtime.seen.lock().unwrap().clone();
        seen.sort_by_key(|item| item.0);
        assert_eq!(
            seen,
            vec![(1, b"reads-a".to_vec()), (2, b"reads-b".to_vec())]
        );
        drop(seen);
        let manifest_path = plugin.join("manifest.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("arity=\"2\"", "arity=\"3\"");
        std::fs::write(manifest_path, manifest).unwrap();
        let engine = DataEngine::builder()
            .with_plugins_root(plugins.clone())
            .with_container_execution(infra.clone())
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let edit = client
            .run_dag_shell(script.clone(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(!report.ok);
        assert!(
            report.instances.iter().any(|instance| instance
                .error
                .as_deref()
                .is_some_and(|error| error.contains("incorrect number of output files"))),
            "{:?}",
            report.instances
        );
        let manifest_path = plugin.join("manifest.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("arity=\"3\"", "arity=\"2\"")
            .replace("arity=\"1\"", "arity=\"2\"");
        std::fs::write(manifest_path, manifest).unwrap();
        let engine = DataEngine::builder()
            .with_plugins_root(plugins)
            .with_container_execution(infra)
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let error = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .expect_err("one input file cannot satisfy arity 2");
        assert!(
            error
                .to_string()
                .contains("incorrect number of input files"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn shell_tuple_script_requires_a_registered_factory() {
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let script = "from_list('reads', [[1, 'reads.fastq']])\nnode('qc', 'python_script', {'code':'pass', 'inputs':[{'name':'pair','kind':'tuple','tuple':[{'val':'sample'},{'path':'reads'}]}], 'outputs':[{'name':'out','kind':'file','format':'txt'}]})\nedge('reads', 0, 'qc', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let error = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .expect_err("the default registry intentionally excludes script nodes");
        assert!(error.to_string().contains("python_script"), "{error}");
    }

    #[tokio::test]
    async fn shell_edits_flow_transactionally_and_supports_dry_run() {
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let script = "from_list('reads', ['a', 'b'])\nfor name in ['qc', 'align']:\n    node(name, 'echo', {})\nedge('reads', 0, 'qc', 0)\nedge('qc', 0, 'align', 0)\ncommit()\n";
        let preview = client
            .run_dag_shell(script.into(), true, None)
            .await
            .unwrap();
        assert!(preview.ok, "{:?}", preview.error);
        assert!(!preview.applied);
        assert!(client.get_flow().await.unwrap().is_empty());
        let done = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(done.ok, "{:?}", done.error);
        let graph = client.get_flow().await.unwrap();
        assert_eq!(graph.processes().len(), 2);
        assert_eq!(graph.sources().len(), 1);
        assert_eq!(graph.wires().len(), 2);
        let invalid = client
            .run_dag_shell("node('qc', 'echo', {})\ncommit()".into(), false, None)
            .await
            .unwrap();
        assert!(!invalid.ok);
        assert_eq!(client.get_flow().await.unwrap().processes().len(), 2);
        let edit = client
            .run_dag_shell(
                "node('sink', 'echo', {})\ncollect_edge('align', 0, 'sink', 0)\ncommit()".into(),
                false,
                None,
            )
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        assert_eq!(client.get_flow().await.unwrap().wires().len(), 3);
    }
    #[tokio::test]
    async fn shell_constructed_channel_flow_runs_from_session() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let items = serde_json::to_string(&vec![a.to_string_lossy(), b.to_string_lossy()]).unwrap();
        let script = format!(
            "from_list('files', {items})\nnode('read', 'file_reference', {{'path': '{{{{item}}}}'}})\nedge('files', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }
    #[tokio::test]
    async fn shell_filter_drops_values_before_downstream_execution() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let items = serde_json::to_string(&vec![
            serde_json::json!({"path":a.to_string_lossy(), "keep":1}),
            serde_json::json!({"path":b.to_string_lossy(), "keep":0}),
        ])
        .unwrap();
        let script = format!(
            "from_list('samples', {items})\nnode('filter', 'channel', {{'operator': 'filter', 'path': 'keep', 'equals': 1}})\nnode('read', 'file_reference', {{'path': '{{{{item.path}}}}'}})\nedge('samples', 0, 'filter', 0)\nedge('filter', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["filter"].succeeded, 2);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_from_path_options_control_vfs_enumeration() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for path in ["data/a.fastq", "data/.hidden.fastq"] {
            storage
                .write_bytes(path, b"content".to_vec())
                .await
                .unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "from_path('reads', 'data/*.fastq', hidden=True, checkIfExists=True)\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('reads', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_from_path_max_depth_limits_recursive_batch() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for path in ["data/root.fastq", "data/a/one.fastq", "data/a/b/two.fastq"] {
            storage
                .write_bytes(path, b"content".to_vec())
                .await
                .unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "from_path('reads', 'data/**/*.fastq', maxDepth=1)\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('reads', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_from_path_accepts_multiple_overlapping_patterns() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for path in ["data/a.fastq", "data/b.fastq"] {
            storage
                .write_bytes(path, b"content".to_vec())
                .await
                .unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "from_path('reads', ['data/a.fastq', 'data/*.fastq'])\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('reads', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }

    #[tokio::test]
    async fn shell_from_path_rejects_unsupported_or_invalid_options() {
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        for script in [
            "from_path('reads', 'data/*', hidden='yes')\ncommit()",
            "from_path('reads', 'data/*', typo=True)\ncommit()",
            "from_path('reads', 'data/*', maxDepth=True)\ncommit()",
        ] {
            let edit = client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap();
            assert!(!edit.ok, "{script}");
        }
    }
    #[tokio::test]
    async fn shell_channel_source_and_flatten_emit_individual_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let items =
            serde_json::to_string(&vec![vec![a.to_string_lossy(), b.to_string_lossy()]]).unwrap();
        let script = format!(
            "node('source', 'channel', {{'operator':'of_items', 'items':{items}}})\nnode('flatten', 'channel', {{'operator':'flatten'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('source', 0, 'flatten', 0)\nedge('flatten', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["source"].succeeded, 1);
        assert_eq!(report.processes["flatten"].succeeded, 1);
        assert_eq!(report.processes["read"].succeeded, 2);
    }
    #[tokio::test]
    async fn shell_mix_forwards_both_inputs_without_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let a = serde_json::to_string(&paths[..2]).unwrap();
        let b = serde_json::to_string(&paths[2..]).unwrap();
        let script = format!(
            "from_list('a', {a})\nfrom_list('b', {b})\nnode('mix', 'channel', {{'operator':'mix'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('a', 0, 'mix', 0)\nedge('b', 0, 'mix', 1)\nedge('mix', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }
    #[tokio::test]
    async fn shell_concat_delivers_each_source_to_downstream() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let a = serde_json::to_string(&paths[..2]).unwrap();
        let b = serde_json::to_string(&paths[2..]).unwrap();
        let script = format!(
            "from_list('a', {a})\nfrom_list('b', {b})\nnode('cat', 'channel', {{'operator':'concat'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('b', 0, 'cat', 1)\nedge('a', 0, 'cat', 0)\nedge('cat', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }

    #[tokio::test]
    async fn shell_combine_emits_all_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let files = serde_json::to_string(&paths).unwrap();
        let script = format!(
            "from_list('files', {files})\nfrom_list('params', [1, 2, 3])\nnode('combine', 'channel', {{'operator':'combine'}})\nnode('read', 'file_reference', {{'path':'{{{{item.0}}}}'}})\nedge('files', 0, 'combine', 0)\nedge('params', 0, 'combine', 1)\nedge('combine', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 6);
    }
    #[tokio::test]
    async fn shell_join_pairs_duplicate_keys_one_to_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let left = serde_json::to_string(&vec![
            serde_json::json!(["k", paths[0]]),
            serde_json::json!(["k", paths[1]]),
            serde_json::json!(["unmatched", paths[2]]),
        ])
        .unwrap();
        let script = format!(
            "from_list('left', {left})\nfrom_list('right', [['k', 1]])\nnode('join', 'channel', {{'operator':'join'}})\nnode('read', 'file_reference', {{'path':'{{{{item.1}}}}'}})\nedge('left', 0, 'join', 0)\nedge('right', 0, 'join', 1)\nedge('join', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_collect_ignores_empty_lists_before_closing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.txt");
        std::fs::write(&path, "data").unwrap();
        for (items, expected) in [
            (serde_json::json!([[], []]), 0),
            (
                serde_json::json!([[], [path.to_string_lossy().to_string()]]),
                1,
            ),
        ] {
            let script = format!(
                "from_list('items', {items})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\ncollect_edge('items', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_unique_and_distinct_consume_a_stream() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let items = serde_json::json!([
            a.to_string_lossy(),
            a.to_string_lossy(),
            b.to_string_lossy(),
            a.to_string_lossy()
        ]);
        for (operator, expected) in [("unique", 2), ("distinct", 3)] {
            let script = format!(
                "from_list('items', {items})\nnode('dedup', 'channel', {{'operator':'{operator}'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'dedup', 0)\nedge('dedup', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_first_and_take_limit_task_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for name in ["a", "b", "c"] {
            let path = dir.path().join(name);
            std::fs::write(&path, name).unwrap();
            paths.push(path.to_string_lossy().into_owned());
        }
        let items = serde_json::to_string(&paths).unwrap();
        for (options, expected) in [
            ("'operator':'first'", 1),
            ("'operator':'take', 'count':2", 2),
            ("'operator':'take', 'count':0", 0),
        ] {
            let script = format!(
                "from_list('items', {items})\nnode('limit', 'channel', {{{options}}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'limit', 0)\nedge('limit', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_last_and_count_emit_for_empty_and_nonempty_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.txt");
        std::fs::write(&path, "data").unwrap();
        let path = serde_json::to_string(&path.to_string_lossy().to_string()).unwrap();
        for operator in ["last", "count"] {
            for items in ["[]", "[1, 2]"] {
                let script = format!(
                    "from_list('items', {items})\nnode('reduce', 'channel', {{'operator':'{operator}'}})\nnode('read', 'file_reference', {{'path':{path}}})\nedge('items', 0, 'reduce', 0)\nedge('reduce', 0, 'read', 0)\ncommit()\n"
                );
                let (client, _handle) =
                    data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
                let edit = client.run_dag_shell(script, false, None).await.unwrap();
                assert!(edit.ok, "{:?}", edit.error);
                let report = client
                    .run_flow(None, data_engine::flow::FlowOptions::default())
                    .await
                    .unwrap();
                assert!(report.ok, "{:?}", report.instances);
                assert_eq!(report.processes["read"].succeeded, 1);
            }
        }
    }
    #[tokio::test]
    async fn shell_group_tuple_collects_by_key_until_close() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let items = serde_json::to_string(&vec![
            serde_json::json!(["g", paths[0]]),
            serde_json::json!(["g", paths[1]]),
            serde_json::json!(["h", paths[2]]),
        ])
        .unwrap();
        let script = format!(
            "from_list('samples', {items})\nnode('group', 'channel', {{'operator':'group_tuple'}})\nnode('read', 'file_reference', {{'path':'{{{{item.1.0}}}}'}})\nedge('samples', 0, 'group', 0)\nedge('group', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }
    #[tokio::test]
    async fn shell_group_tuple_natural_sort_and_null_by() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a");
        let missing = dir.path().join("z");
        std::fs::write(&first, "data").unwrap();
        let items = serde_json::to_string(&serde_json::json!([
            ["sample", missing.to_string_lossy()],
            ["sample", first.to_string_lossy()]
        ]))
        .unwrap();
        for sort in ["True", "'natural'"] {
            let script = format!(
                "from_list('items', {items})\nnode('group', 'channel', {{'operator':'group_tuple', 'by':None, 'size':2, 'sort':{sort}}})\nnode('read', 'file_reference', {{'path':'{{{{item.1.0}}}}'}})\nedge('items', 0, 'group', 0)\nedge('group', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_group_tuple_size_controls_remainder() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let items = serde_json::to_string(
            &paths
                .iter()
                .map(|path| serde_json::json!(["g", path]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        for (remainder, expected) in [(false, 1), (true, 2)] {
            let flag = if remainder { "True" } else { "False" };
            let script = format!(
                "from_list('samples', {items})\nnode('group', 'channel', {{'operator':'group_tuple', 'size':2, 'remainder':{flag}}})\nnode('read', 'file_reference', {{'path':'{{{{item.1.0}}}}'}})\nedge('samples', 0, 'group', 0)\nedge('group', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }
    #[tokio::test]
    async fn shell_join_remainder_and_mismatch_options() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let left = serde_json::to_string(&vec![
            serde_json::json!(["k", paths[0]]),
            serde_json::json!(["other", paths[1]]),
        ])
        .unwrap();
        for (options, expected) in [
            ("'remainder': True", true),
            ("'failOnMismatch': True", false),
        ] {
            let script = format!(
                "from_list('left', {left})\nfrom_list('right', [['k', 1]])\nnode('join', 'channel', {{'operator':'join', {options}}})\nnode('read', 'file_reference', {{'path':'{{{{item.1}}}}'}})\nedge('left', 0, 'join', 0)\nedge('right', 0, 'join', 1)\nedge('join', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let result = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await;
            if expected {
                let report = result.unwrap();
                assert!(report.ok);
                assert_eq!(report.processes["read"].succeeded, 2);
            } else {
                assert!(result.unwrap_err().to_string().contains("join mismatch"));
            }
        }
    }
    #[tokio::test]
    async fn shell_join_fail_on_duplicate_reports_duplicate() {
        let script = "from_list('left', [['k', 1], ['k', 2]])\nfrom_list('right', [['k', 3]])\nnode('join', 'channel', {'operator':'join', 'failOnDuplicate':True})\nedge('left', 0, 'join', 0)\nedge('right', 0, 'join', 1)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let error = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("join duplicate"), "{error}");
    }
    #[tokio::test]
    async fn shell_combine_by_only_emits_matching_key_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let left = serde_json::to_string(&vec![
            serde_json::json!(["g", paths[0]]),
            serde_json::json!(["h", paths[1]]),
        ])
        .unwrap();
        let script = format!(
            "from_list('left', {left})\nfrom_list('right', [['g', 1], ['g', 2], ['z', 3]])\nnode('combine', 'channel', {{'operator':'combine', 'by':0}})\nnode('read', 'file_reference', {{'path':'{{{{item.1}}}}'}})\nedge('left', 0, 'combine', 0)\nedge('right', 0, 'combine', 1)\nedge('combine', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }
    #[tokio::test]
    async fn shell_flatten_recurses_and_value_channel_produces_a_queue() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let p = dir.path().join(name);
                std::fs::write(&p, name).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let nested =
            serde_json::to_string(&serde_json::json!([paths[0], [paths[1], [paths[2]]]])).unwrap();
        let script = format!(
            "channel_value('value', {nested})\nnode('flatten', 'channel', {{'operator':'flatten'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('value', 0, 'flatten', 0)\nedge('flatten', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }

    #[tokio::test]
    async fn shell_map_does_not_emit_null_results() {
        let script = "from_list('values', [1, 2])\nnode('map', 'channel', {'operator':'map', 'template':None})\nnode('sink', 'echo', {})\nedge('values', 0, 'map', 0)\nedge('map', 0, 'sink', 0)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 0);
    }
    #[tokio::test]
    async fn shell_collect_is_composable_and_respects_flat_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample");
        std::fs::write(&file, "data").unwrap();
        let path = serde_json::to_string(&file.to_string_lossy()).unwrap();
        for (flat, item_path) in [("True", "{{item.0}}"), ("False", "{{item.0.0}}")] {
            let script = format!(
                "from_list('items', [[{path}]])\nnode('collect', 'channel', {{'operator':'collect', 'flat':{flat}}})\nnode('read', 'file_reference', {{'path':'{item_path}'}})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
        for values in ["[]", "[[]]"] {
            let script = format!(
                "from_list('items', {values})\nnode('collect', 'channel', {{'operator':'collect'}})\nnode('sink', 'channel', {{'operator':'map', 'template':'ok'}})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'sink', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            assert!(client.run_dag_shell(script, false, None).await.unwrap().ok);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].total, 0);
        }
    }

    #[tokio::test]
    async fn shell_collect_applies_function_before_flattening() {
        for (flat, expected) in [
            ("True", "[1, 11, None, 3, 13]"),
            ("False", "[[1, 11], None, [3, 13]]"),
        ] {
            let script = format!(
                "def transform(item):\n    if item == 2:\n        return None\n    return [item, item + 10]\ndef check(items):\n    return items == {expected}\nfrom_list('items', [1, 2, 3])\nnode('collect', 'channel', {{'operator':'collect', 'function':transform, 'flat':{flat}}})\nnode('check', 'channel', {{'operator':'filter', 'function':check}})\nnode('sink', 'channel', {{'operator':'map', 'template':'ok'}})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'check', 0)\nedge('check', 0, 'sink', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].total, 1);
        }
        let script = "def fail(item):\n    return 1 / 0\nfrom_list('items', [1])\nnode('collect', 'channel', {'operator':'collect', 'function':fail})\nedge('items', 0, 'collect', 0)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        assert!(
            client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        assert!(
            client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shell_binds_process_inputs_without_edges_across_edits() {
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let preview = "reads = from_list('preview_source', [1])\nprocess('preview', 'channel', {'operator':'map', 'template':'ok'}, inputs=[reads])\ncommit()\n";
        let dry_run = client
            .run_dag_shell(preview.into(), true, None)
            .await
            .unwrap();
        assert!(dry_run.ok && !dry_run.applied);
        assert!(
            client
                .get_flow_view()
                .await
                .unwrap()
                .flow
                .process("preview")
                .is_none()
        );
        let first = "reads = from_list('reads', [1, 2, 3])\ndef twice(item):\n    return item * 2\nprocess('double', 'channel', {'operator':'map', 'function':twice}, inputs=[reads])\ncommit()\n";
        let edit = client
            .run_dag_shell(first.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let second = "values = output('double')\ndef check(item):\n    return item == 4\nmatched = process('match', 'channel', {'operator':'filter', 'function':check}, inputs=[values])\nprocess('sink', 'channel', {'operator':'map', 'template':'ok'}, inputs=[matched])\ncommit()\n";
        let edit = client
            .run_dag_shell(second.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 1);
        let rebind = "replacement = from_list('replacement', [1])\nbind_inputs('double', [replacement])\ncommit()\n";
        let edit = client
            .run_dag_shell(rebind.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 0);
    }

    #[tokio::test]
    async fn shell_binds_file_source_to_template_node() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        storage
            .write_bytes("data/a.txt", b"one".to_vec())
            .await
            .unwrap();
        storage
            .write_bytes("data/b.txt", b"two".to_vec())
            .await
            .unwrap();
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "reads = from_path('reads', 'data/*.txt')\nprocess('load', kind='file_reference', spec={'path':'{{item}}'}, inputs=[reads])\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["load"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_binds_named_outputs_and_rejects_invalid_dependencies() {
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let script = "def good(item):\n    return item == 1\ndef other(item):\n    return True\nvalues = from_list('values', [1, 2])\nparts = process('split', 'channel', {'operator':'branch', 'branches':[{'name':'good', 'function':good}, {'name':'other', 'function':other}]}, inputs=[values])\nprocess('sink', 'channel', {'operator':'map', 'template':'ok'}, inputs=[parts.good])\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 1);
        let bad = "parts = output('split')\nprocess('bad', 'channel', {'operator':'map', 'template':'x'}, inputs=[parts.missing])\ncommit()\n";
        assert!(
            !client
                .run_dag_shell(bad.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        let view = client.get_flow_view().await.unwrap();
        assert!(view.flow.process("bad").is_none());
    }

    #[tokio::test]
    async fn shell_collect_sorts_by_function_without_changing_values() {
        let script = "def key(item):\n    return len(item)\ndef check(items):\n    return items == ['c', 'bb', 'dd', 'aaa']\nfrom_list('items', ['aaa', 'bb', 'dd', 'c'])\nnode('collect', 'channel', {'operator':'collect', 'sort':key})\nnode('check', 'channel', {'operator':'filter', 'function':check})\nnode('sink', 'channel', {'operator':'map', 'template':'ok'})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'check', 0)\nedge('check', 0, 'sink', 0)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 1);

        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        storage
            .write_bytes("data/aaa.dat", b"x".to_vec())
            .await
            .unwrap();
        storage
            .write_bytes("data/c.dat", b"x".to_vec())
            .await
            .unwrap();
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def key(item):\n    return len(item.name)\ndef check(items):\n    return items[0].name == 'c.dat' and items[1].name == 'aaa.dat'\nfrom_path('items', 'data/*.dat')\nnode('collect', 'channel', {'operator':'collect', 'sort':key})\nnode('check', 'channel', {'operator':'filter', 'function':check})\nnode('sink', 'channel', {'operator':'map', 'template':'ok'})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'check', 0)\nedge('check', 0, 'sink', 0)\ncommit()\n";
        assert!(
            client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].total, 1);

        let script = "def fail(item):\n    return 1 / 0\nfrom_list('items', [1])\nnode('collect', 'channel', {'operator':'collect', 'sort':fail})\nedge('items', 0, 'collect', 0)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        assert!(
            client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        assert!(
            client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shell_collect_and_to_sorted_list_sort_before_emitting() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a");
        let missing = dir.path().join("z");
        std::fs::write(&first, "data").unwrap();
        let values =
            serde_json::to_string(&vec![missing.to_string_lossy(), first.to_string_lossy()])
                .unwrap();
        for spec in [
            "{'operator':'collect', 'sort':True}",
            "{'operator':'to_sorted_list'}",
        ] {
            let script = format!(
                "from_list('items', {values})\nnode('sort', 'channel', {spec})\nnode('read', 'file_reference', {{'path':'{{{{item.0}}}}'}})\nedge('items', 0, 'sort', 0)\nedge('sort', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_buffer_emits_batches_and_optional_remainder() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..5)
            .map(|n| {
                let path = dir.path().join(n.to_string());
                std::fs::write(&path, "data").unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let values = serde_json::to_string(&paths).unwrap();
        for (remainder, expected) in [("False", 2), ("True", 3)] {
            let script = format!(
                "from_list('items', {values})\nnode('batch', 'channel', {{'operator':'buffer', 'size':2, 'remainder':{remainder}}})\nnode('read', 'file_reference', {{'path':'{{{{item.0}}}}'}})\nedge('items', 0, 'batch', 0)\nedge('batch', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_if_empty_forwards_values_and_supplies_fallback_only_for_empty_input() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let values =
            serde_json::to_string(&vec![a.to_string_lossy(), b.to_string_lossy()]).unwrap();
        let fallback = serde_json::to_string(&a.to_string_lossy()).unwrap();
        for (items, expected) in [("[]", 1), (values.as_str(), 2)] {
            let script = format!(
                "from_list('items', {items})\nnode('default', 'channel', {{'operator':'if_empty', 'value':{fallback}}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'default', 0)\nedge('default', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
        let script = format!(
            "from_list('items', [])\nnode('collect', 'channel', {{'operator':'collect'}})\nnode('default', 'channel', {{'operator':'if_empty', 'value':{fallback}}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'collect', 0)\nedge('collect', 0, 'default', 0)\nedge('default', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_flat_map_splits_each_mapped_list() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let rows = serde_json::to_string(&serde_json::json!([{"paths":paths}])).unwrap();
        let script = format!(
            "from_list('items', {rows})\nnode('split', 'channel', {{'operator':'flat_map', 'template':'{{{{item.paths}}}}'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'split', 0)\nedge('split', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }

    #[tokio::test]
    async fn shell_transpose_expands_grouped_tuple_to_file_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let rows = serde_json::to_string(&serde_json::json!([["sample", paths]])).unwrap();
        let script = format!(
            "from_list('items', {rows})\nnode('transpose', 'channel', {{'operator':'transpose'}})\nnode('read', 'file_reference', {{'path':'{{{{item.1}}}}'}})\nedge('items', 0, 'transpose', 0)\nedge('transpose', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 3);
    }

    #[tokio::test]
    async fn shell_if_empty_evaluates_fallback_lazily() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid");
        std::fs::write(&path, "data").unwrap();
        let path = serde_json::to_string(&path.to_string_lossy()).unwrap();
        for (items, body) in [
            ("[]".to_string(), "return fallback"),
            (format!("[{path}]"), "return 1 / 0"),
        ] {
            let script = format!(
                "fallback = {path}\ndef choose():\n    {body}\nfrom_list('items', {items})\nnode('default', 'channel', {{'operator':'if_empty', 'value':choose}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'default', 0)\nedge('default', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_first_and_count_use_function_predicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid");
        let missing = dir.path().join("missing");
        std::fs::write(&path, "data").unwrap();
        let items = serde_json::to_string(&vec![missing.to_string_lossy(), path.to_string_lossy()])
            .unwrap();
        let wanted = serde_json::to_string(&path.to_string_lossy()).unwrap();
        let script = format!(
            "wanted = {wanted}\ndef matches(item):\n    return item == wanted\ndef is_one(item):\n    return item == 1\nfrom_list('items', {items})\nnode('first', 'channel', {{'operator':'first', 'criteria':matches}})\nnode('count', 'channel', {{'operator':'count', 'criteria':matches}})\nnode('check', 'channel', {{'operator':'filter', 'function':is_one}})\nnode('sink', 'channel', {{'operator':'map', 'template':'ok'}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'first', 0)\nedge('items', 0, 'count', 0)\nedge('first', 0, 'read', 0)\nedge('count', 0, 'check', 0)\nedge('check', 0, 'sink', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
        assert_eq!(report.processes["sink"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_collect_preserves_files_inside_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        for (operator, access) in [
            ("{'operator':'to_list'}", "items[0][1]"),
            ("{'operator':'collect', 'flat':False}", "items[0][1]"),
            ("{'operator':'collect'}", "items[1]"),
        ] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def pair(item):\n    return ('sample', item)\ndef extract(items):\n    return {access}\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {{'operator':'map', 'function':pair}})\nnode('collect', 'channel', {operator})\nnode('extract', 'channel', {{'operator':'map', 'function':extract}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'collect', 0)\nedge('collect', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n"
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_from_file_pairs_checks_missing_and_literal_patterns() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        storage
            .write_bytes("data/literal*.dat", b"data".to_vec())
            .await
            .unwrap();
        let engine = DataEngine::builder()
            .register_opendal_fs(storage.clone())
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def first(pair):\n    return pair[1][0]\nfrom_file_pairs('files', 'data/literal*.dat', glob=False, size=1, checkIfExists=True)\nnode('first', 'channel', {'operator':'map', 'function':first})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'first', 0)\nedge('first', 0, 'read', 0)\ncommit()\n";
        assert!(
            client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "from_file_pairs('files', 'data/missing*.dat', checkIfExists=True)\nnode('read', 'file_reference', {'path':'{{item.1.0}}'})\nedge('files', 0, 'read', 0)\ncommit()\n";
        assert!(
            client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap()
                .ok
        );
        let error = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no files match"), "{error}");
    }

    #[tokio::test]
    async fn shell_from_file_pairs_accepts_grouping_function() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in [
            "data/sample_R1.fastq",
            "data/sample_R2.fastq",
            "data/other_R1.fastq",
        ] {
            storage.write_bytes(name, b"reads".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def group(path):\n    return (path.name.split('_')[0], 42)\ndef valid(pair):\n    return pair[0][0] == 'sample' and pair[0][1] == 42 and len(pair[1]) == 2\ndef extract(pair):\n    return pair[1][0]\nfrom_file_pairs('reads', 'data/*_R{1,2}.fastq', grouping=group)\nnode('valid', 'channel', {'operator':'filter', 'function':valid})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('reads', 0, 'valid', 0)\nedge('valid', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_from_file_pairs_groups_files_without_expanding_the_graph() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in [
            "data/sample_R1.fastq",
            "data/sample_R2.fastq",
            "data/incomplete_R1.fastq",
        ] {
            storage.write_bytes(name, b"reads".to_vec()).await.unwrap();
        }
        for (flat, read_expr, size, expected) in [
            (false, "pair[1][0]", "", 1),
            (true, "pair[1]", "", 1),
            (false, "pair[1][0]", ", size=-1", 2),
        ] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def extract(pair):\n    return {read_expr}\nfrom_file_pairs('reads', 'data/*_R{{1,2}}.fastq', flat={}{size})\nnode('extract', 'channel', {{'operator':'map', 'function':extract}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('reads', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n",
                if flat { "True" } else { "False" }
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_reduce_preserves_file_accumulator() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def keep(first, item):\n    return first\nfrom_path('files', 'data/*.dat')\nnode('reduce', 'channel', {'operator':'reduce', 'function':keep})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'reduce', 0)\nedge('reduce', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_reduce_supports_two_argument_functions_and_seed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.txt");
        std::fs::write(&path, "data").unwrap();
        let path = serde_json::to_string(&path.to_string_lossy()).unwrap();
        for (items, seed, expected) in [
            ("[2,3,4]", ", 'seed':1", "1234"),
            ("[2,3,4]", "", "234"),
            ("[]", ", 'seed':7", "7"),
            ("[]", "", "None"),
        ] {
            let script = format!(
                "def add(acc, item):\n    return acc * 10 + item\ndef correct(item):\n    return item == {expected}\nfrom_list('items', {items})\nnode('reduce', 'channel', {{'operator':'reduce', 'function':add{seed}}})\nnode('check', 'channel', {{'operator':'filter', 'function':correct}})\nnode('path', 'channel', {{'operator':'map', 'template':{path}}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'reduce', 0)\nedge('reduce', 0, 'check', 0)\nedge('check', 0, 'path', 0)\nedge('path', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_until_stops_before_matching_file() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat", "data/c.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def stop(item):\n    return item.name == 'b.dat'\nfrom_path('files', 'data/*.dat')\nnode('until', 'channel', {'operator':'until', 'function':stop})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'until', 0)\nedge('until', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_unique_and_distinct_support_function_keys_for_file_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        for (operator, expected) in [("unique", 2), ("distinct", 4)] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def pair(item):\n    return (item.name, item)\ndef key(item):\n    return item[0]\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {{'operator':'map', 'function':pair}})\nnode('repeat', 'channel', {{'operator':'concat'}})\nnode('dedup', 'channel', {{'operator':'{operator}', 'function':key}})\nnode('extract', 'channel', {{'operator':'map', 'function':extract}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'repeat', 0)\nedge('pair', 0, 'repeat', 1)\nedge('repeat', 0, 'dedup', 0)\nedge('dedup', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n"
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_branch_functions_route_and_transform_files() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef first(item):\n    return item[1].name == 'a.dat'\ndef otherwise(item):\n    return True\ndef output(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('branch', 'channel', {'operator':'branch', 'branches':[{'name':'first', 'function':first, 'transform':output}, {'name':'other', 'function':otherwise, 'transform':output}]})\nnode('read_a', 'file_reference', {'path':'{{item}}'})\nnode('read_b', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'branch', 0)\nedge('branch', 0, 'read_a', 0)\nedge('branch', 1, 'read_b', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read_a"].succeeded, 1);
        assert_eq!(report.processes["read_b"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_branch_preserves_files_in_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('branch', 'channel', {'operator':'branch', 'branches':[{'name':'matched', 'path':'0', 'equals':'sample'}, {'name':'other', 'path':'0', 'not_equals':'sample'}]})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'branch', 0)\nedge('branch', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_transpose_expands_grouped_file_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('group', 'channel', {'operator':'group_tuple'})\nnode('transpose', 'channel', {'operator':'transpose'})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'group', 0)\nedge('group', 0, 'transpose', 0)\nedge('transpose', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_buffer_and_flatten_preserve_file_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let scripts = [
            "def pair(item):\n    return ('sample', item)\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('buffer', 'channel', {'operator':'buffer', 'size':2})\nnode('split', 'channel', {'operator':'flat_map'})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'buffer', 0)\nedge('buffer', 0, 'split', 0)\nedge('split', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n",
            "def nested(item):\n    return (['sample'], [item])\ndef keep(item):\n    return item != 'sample'\nfrom_path('files', 'data/*.dat')\nnode('nested', 'channel', {'operator':'map', 'function':nested})\nnode('flatten', 'channel', {'operator':'flatten'})\nnode('keep', 'channel', {'operator':'filter', 'function':keep})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'nested', 0)\nedge('nested', 0, 'flatten', 0)\nedge('flatten', 0, 'keep', 0)\nedge('keep', 0, 'read', 0)\ncommit()\n",
        ];
        for script in scripts {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let edit = client
                .run_dag_shell(script.into(), false, None)
                .await
                .unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 2);
        }
    }

    #[tokio::test]
    async fn shell_merge_and_unique_preserve_file_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        for operator in ["mix", "concat"] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def pair(item):\n    return ('sample', item)\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {{'operator':'map', 'function':pair}})\nnode('merge', 'channel', {{'operator':'{operator}'}})\nnode('unique', 'channel', {{'operator':'unique'}})\nnode('extract', 'channel', {{'operator':'map', 'function':extract}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'merge', 0)\nedge('pair', 0, 'merge', 1)\nedge('merge', 0, 'unique', 0)\nedge('unique', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n"
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 2);
        }
    }

    #[tokio::test]
    async fn shell_join_and_combine_preserve_files_in_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/l1.dat", "data/l2.dat", "data/r1.dat", "data/r2.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        for (operator, expected) in [
            ("{'operator':'join'}", 2),
            ("{'operator':'combine', 'by':0}", 4),
        ] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def pair(item):\n    return ('sample', item)\ndef left(row):\n    return row[1]\nfrom_path('left_files', 'data/l*.dat')\nfrom_path('right_files', 'data/r*.dat')\nnode('left', 'channel', {{'operator':'map', 'function':pair}})\nnode('right', 'channel', {{'operator':'map', 'function':pair}})\nnode('merge', 'channel', {operator})\nnode('extract', 'channel', {{'operator':'map', 'function':left}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('left_files', 0, 'left', 0)\nedge('right_files', 0, 'right', 0)\nedge('left', 0, 'merge', 0)\nedge('right', 0, 'merge', 1)\nedge('merge', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n"
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_group_tuple_preserves_file_values() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/z.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef first(group):\n    return group[1][0]\ndef is_first(item):\n    return item.name == 'a.dat'\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('group', 'channel', {'operator':'group_tuple', 'sort':True})\nnode('first', 'channel', {'operator':'map', 'function':first})\nnode('check', 'channel', {'operator':'filter', 'function':is_first})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'group', 0)\nedge('group', 0, 'first', 0)\nedge('first', 0, 'check', 0)\nedge('check', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_sorted_collection_preserves_file_values() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/z.dat", "data/a.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        for operator in [
            "{'operator':'to_sorted_list'}",
            "{'operator':'collect', 'sort':True}",
        ] {
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let script = format!(
                "def first(items):\n    return items[0]\ndef is_first(item):\n    return item.name == 'a.dat'\nfrom_path('files', 'data/*.dat')\nnode('sort', 'channel', {operator})\nnode('first', 'channel', {{'operator':'map', 'function':first}})\nnode('check', 'channel', {{'operator':'filter', 'function':is_first}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('files', 0, 'sort', 0)\nedge('sort', 0, 'first', 0)\nedge('first', 0, 'check', 0)\nedge('check', 0, 'read', 0)\ncommit()\n"
            );
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn shell_collect_edge_preserves_files_inside_tuples() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef extract(items):\n    return items[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\ncollect_edge('pair', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_mixed_tuple_preserves_file_through_functions() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/good.dat", "data/other.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def pair(item):\n    return ('sample', item)\ndef keep(item):\n    return item[0] == 'sample' and item[1].name == 'good.dat'\ndef extract(item):\n    return item[1]\nfrom_path('files', 'data/*.dat')\nnode('pair', 'channel', {'operator':'map', 'function':pair})\nnode('keep', 'channel', {'operator':'filter', 'function':keep})\nnode('extract', 'channel', {'operator':'map', 'function':extract})\nnode('read', 'file_reference', {'path':'{{item}}'})\nedge('files', 0, 'pair', 0)\nedge('pair', 0, 'keep', 0)\nedge('keep', 0, 'extract', 0)\nedge('extract', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_flat_map_without_function_preserves_files() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def keep(item):\n    return item.name == 'a.dat'\nfrom_path('files', 'data/*.dat')\nnode('split', 'channel', {'operator':'flat_map'})\nnode('filter', 'channel', {'operator':'filter', 'function':keep})\nnode('read', 'file_reference', {'path':'{{item}}'})\ncollect_edge('files', 0, 'split', 0)\nedge('split', 0, 'filter', 0)\nedge('filter', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_function_preserves_and_expands_file_sets() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        for name in ["data/a.dat", "data/b.dat"] {
            storage.write_bytes(name, b"data".to_vec()).await.unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
        let script = "def identity(item):\n    return item\nfrom_path('files', 'data/*.dat')\nnode('copy', 'channel', {'operator':'map', 'function':identity})\nnode('split', 'channel', {'operator':'flat_map', 'function':identity})\nnode('read', 'file_reference', {'path':'{{item}}'})\ncollect_edge('files', 0, 'copy', 0)\nedge('copy', 0, 'split', 0)\nedge('split', 0, 'read', 0)\ncommit()\n";
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_channel_function_captures_other_functions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sample.txt"), "data").unwrap();
        let prefix = serde_json::to_string(&format!("{}/", dir.path().display())).unwrap();
        let script = format!(
            "prefix = {prefix}\nsuffix = '.txt'\ndef helper(item, ext=suffix):\n    if item == 'sample':\n        return helper('done', ext)\n    return prefix + 'sample' + ext\nsuffix = '.wrong'\ndef unused(item):\n    return item\ndef transform(item):\n    return helper(item)\nfrom_list('items', ['sample'])\nnode('map', 'channel', {{'operator':'map', 'function':transform}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'map', 0)\nedge('map', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_functions_accept_from_path_files() {
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        storage
            .write_bytes("data/good.dat", b"a".to_vec())
            .await
            .unwrap();
        storage
            .write_bytes("data/other.dat", b"b".to_vec())
            .await
            .unwrap();
        let pattern = "'data/*.dat'";
        for (operator, body, expected) in [
            ("filter", "return item.name == 'good.dat'", 1),
            ("map", "return item", 2),
        ] {
            let script = format!(
                "def transform(item):\n    {body}\nfrom_path('items', {pattern})\nnode('transform', 'channel', {{'operator':'{operator}', 'function':transform}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'transform', 0)\nedge('transform', 0, 'read', 0)\ncommit()\n"
            );
            let engine = DataEngine::builder()
                .register_opendal_fs(storage.clone())
                .unwrap()
                .build();
            let (client, _handle) = data_engine::runtime::spawn_with_engine(engine);
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_flat_map_uses_persisted_function() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid");
        std::fs::write(&path, "data").unwrap();
        let item = serde_json::to_string(&path.to_string_lossy()).unwrap();
        let script = format!(
            "def duplicate(item):\n    return [item, item]\nfrom_list('items', [{item}])\nnode('flat', 'channel', {{'operator':'flat_map', 'function':duplicate}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'flat', 0)\nedge('flat', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }

    #[tokio::test]
    async fn shell_filter_uses_persisted_function() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid");
        let missing = dir.path().join("missing");
        std::fs::write(&path, "data").unwrap();
        let items = serde_json::to_string(&vec![missing.to_string_lossy(), path.to_string_lossy()])
            .unwrap();
        let wanted = serde_json::to_string(&path.to_string_lossy()).unwrap();
        let script = format!(
            "wanted = {wanted}\ndef keep(item):\n    return item == wanted\nfrom_list('items', {items})\nnode('filter', 'channel', {{'operator':'filter', 'function':keep}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'filter', 0)\nedge('filter', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_map_uses_persisted_function() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        std::fs::write(&path, "data").unwrap();
        let directory = serde_json::to_string(&format!("{}/", dir.path().display())).unwrap();
        let script = format!(
            "prefix = {directory}\ndef resolve(item):\n    return prefix + item\nfrom_list('items', ['sample.txt'])\nnode('map', 'channel', {{'operator':'map', 'function':resolve}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'map', 0)\nedge('map', 0, 'read', 0)\ncommit()\n"
        );
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client.run_dag_shell(script, false, None).await.unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 1);
    }

    #[tokio::test]
    async fn shell_first_with_literal_criteria_skips_nonmatches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid");
        let missing = dir.path().join("missing");
        std::fs::write(&path, "data").unwrap();
        let items = serde_json::to_string(&vec![missing.to_string_lossy(), path.to_string_lossy()])
            .unwrap();
        let literal = serde_json::to_string(&path.to_string_lossy()).unwrap();
        for (criteria, expected) in [
            (literal.clone(), 1),
            (format!("[{literal}]"), 1),
            ("'no match'".to_string(), 0),
        ] {
            let script = format!(
                "from_list('items', {items})\nnode('first', 'channel', {{'operator':'first', 'criteria':{criteria}}})\nnode('read', 'file_reference', {{'path':'{{{{item}}}}'}})\nedge('items', 0, 'first', 0)\nedge('first', 0, 'read', 0)\ncommit()\n"
            );
            let (client, _handle) =
                data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
            let edit = client.run_dag_shell(script, false, None).await.unwrap();
            assert!(edit.ok, "{:?}", edit.error);
            let report = client
                .run_flow(None, data_engine::flow::FlowOptions::default())
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["read"].succeeded, expected);
        }
    }

    #[tokio::test]
    async fn shell_to_list_empty_still_runs_downstream() {
        let script = "from_list('items', [])\nnode('list', 'channel', {'operator':'to_list'})\nnode('sink', 'channel', {'operator':'map', 'template':'ok'})\nedge('items', 0, 'list', 0)\nedge('list', 0, 'sink', 0)\ncommit()\n";
        let (client, _handle) =
            data_engine::runtime::spawn_with_engine(DataEngine::builder().build());
        let edit = client
            .run_dag_shell(script.into(), false, None)
            .await
            .unwrap();
        assert!(edit.ok, "{:?}", edit.error);
        let report = client
            .run_flow(None, data_engine::flow::FlowOptions::default())
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].succeeded, 1);
    }
}
