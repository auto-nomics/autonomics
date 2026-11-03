//! Explicit ignored acceptance: four synthetic Slurm containers, no study data.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use container_runtime::{PanelCache, PodmanConnection};
use dag_core::{
    ApptainerSettings, NodeCtx, NodeRegistry, NodeValue, PlacementTaskExecutor, SlurmSettings,
    SshTarget, TaskExecution, TaskExecutor, TaskOutputBinding, TaskResources, TaskSpec,
    TaskSubmission,
    dag::node_event::{JobResult, NodeReporter},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[tokio::test]
#[ignore = "four real synthetic Slurm jobs; requires a private operator config and managed session"]
async fn real_cluster_parameter_dispatch_and_new_plugin_route()
-> Result<(), Box<dyn std::error::Error>> {
    let permanent = PathBuf::from(std::env::var("AUTONOMICS_VALIDATION_PERMANENT")?);
    let cfg: Value = serde_json::from_slice(&std::fs::read(std::env::var(
        "AUTONOMICS_PLUGIN_SMOKE_CONFIG",
    )?)?)?;
    let text = |key: &str| -> Result<&str, Box<dyn std::error::Error>> {
        cfg[key]
            .as_str()
            .ok_or_else(|| format!("missing {key}").into())
    };
    let analysis = std::fs::read(text("analysis_script")?)?;
    if format!("{:x}", Sha256::digest(&analysis)) != text("analysis_sha256")? {
        return Err("pinned analysis script changed".into());
    }
    let manifest_path = PathBuf::from(text("image_manifest")?);
    if format!("{:x}", Sha256::digest(std::fs::read(&manifest_path)?))
        != text("image_manifest_sha256")?
    {
        return Err("image catalog changed".into());
    }
    let ssh = SshTarget::new("/usr/bin/ssh", text("ssh_host")?)
        .with_endpoint(serde_json::from_value(cfg["endpoint"].clone())?);
    let settings = SlurmSettings {
        ssh,
        apptainer: Some(ApptainerSettings::load(&manifest_path)?),
        scratch_dir: text("remote_scratch")?.into(),
        local_artifact_root: permanent.join("artifacts"),
        partition: text("partition")?.into(),
        account: Some(text("account")?.into()),
        gpus: None,
        qos: None,
        mem_per_cpu: "2G".into(),
        default_time: "00:05:00".into(),
        poll_interval: Duration::from_secs(2),
    };
    let runtime = Arc::new(crate::slurm_runtime::SlurmContainerRuntime::new(
        settings,
        permanent.join("workspaces"),
        permanent.join("journal"),
        text("panel_cache")?.into(),
        text("remote_python")?.into(),
    )?);
    // Use a new family with no entries in any static placement whitelist.
    // The method bytes remain the exact hash-verified installed script.
    let raw = format!(
        r#"
schema_version = 1
plugin_name = "synthetic-parameter-dispatch"
[image]
reference = {}
[[nodes]]
kind = "probe"
desc = "Synthetic parameter dispatch"
doc = "Synthetic parameter dispatch"
timeout_secs = 300
[nodes.ports]
outputs = [{{path="bundle.zip"}}, {{path="receipt.json"}}]
[nodes.params]
min_markers = {{type="int",default=2}}
min_umi = {{type="int",default=500}}
min_genes = {{type="int",default=3}}
max_mito_fraction = {{type="number",default=0.2}}
mito_prefix = {{type="string",default="mt-"}}
[nodes.command]
interpreter = "python3"
script = {}
[nodes.command.files]
"analysis.sh" = {}
[nodes.resources]
cpus = 1.0
memory = "2G"
read_only_rootfs = true
"#,
        toml::Value::String(text("image")?.into()),
        toml::Value::String(include_str!("synthetic_parameter_probe.py").into()),
        toml::Value::String(String::from_utf8(analysis)?),
    );
    std::fs::write(permanent.join("synthetic-plugin.toml"), &raw)?;
    let plugin = crate::factory::Plugin::new(
        toml::from_str(&raw)?,
        raw.as_bytes(),
        runtime as Arc<dyn PodmanConnection>,
        Arc::new(PanelCache::new(permanent.join("panels"))),
    );
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(Arc::new(vfs::OpendalFileStorage::new(
            permanent.join("objects"),
        ))),
    );
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register_plugin(&plugin);
    // No typed worker exists in this test. Success requires the new container
    // adapter route and the real remote container backend; there is no local
    // container runtime or alternate computation path.
    let placement = PlacementTaskExecutor::new(dag_core::local_task_executor(), None, vec![])?
        .with_remote_container_dispatch();
    let mut reports = BTreeMap::new();
    for (name, values) in [
        ("baseline", json!({})),
        ("marker3", json!({"min_markers":3})),
        ("umi999999", json!({"min_umi":999999})),
        ("genes999999", json!({"min_genes":999999})),
    ] {
        let kind = "synthetic-parameter-dispatch/probe";
        let node = registry.build_node(kind, values.clone())?;
        let identity = node.plugin_identity().cloned();
        let spec = TaskSpec {
            id: name.into(),
            logical_node: None,
            kind: kind.into(),
            spec: values,
            axis: None,
            item_key: None,
            item: None,
            inputs: vec![],
            outputs: (0..2)
                .map(|port| TaskOutputBinding {
                    name: format!("output-{port}"),
                    port,
                    payload: "file".into(),
                    artifact_paths: vec![],
                })
                .collect(),
        };
        let result = placement
            .run(TaskExecution {
                node,
                submission: TaskSubmission {
                    spec,
                    inputs: vec![],
                    resources: TaskResources::default(),
                },
                engine_ctx: Arc::new(ctx.clone()),
                reporter: NodeReporter::noop(),
                cancellation: Default::default(),
            })
            .await;
        let JobResult::Success { outputs, .. } = result else {
            return Err(format!("synthetic case {name} failed: {result:?}").into());
        };
        let Some(NodeValue::File(receipt)) = outputs.get(&1) else {
            return Err("missing synthetic receipt".into());
        };
        let report: Value = serde_json::from_slice(&std::fs::read(&receipt.path)?)?;
        assert_eq!(report["synthetic_only"], true);
        assert!(
            report["cgroup"]
                .as_str()
                .is_some_and(|c| c.contains("job_"))
        );
        reports.insert(name, json!({"receipt":report,"plugin_identity":identity}));
    }
    std::fs::write(
        permanent.join("PARAMETER_DISPATCH_PASS.json"),
        serde_json::to_vec_pretty(&json!({
            "source_revision":dag_core::source_revision(), "cases":reports,
            "scientific_execution":false, "new_plugin_without_whitelist":true,
        }))?,
    )?;
    Ok(())
}
