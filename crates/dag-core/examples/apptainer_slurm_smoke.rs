//! One synthetic job using the production SIF renderer and artifact transport.
//! Run through research_validation_session.py; required evidence is permanent.
//! Remote scratch is retained explicitly, even after independent stop proof.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dag_core::{
    ApptainerContainer, ApptainerMode, ApptainerNetwork, ApptainerSettings, FileRef,
    ProcessTaskOutput, ProcessTaskSpec, SlurmSettings, SlurmTaskTransport, SshArtifactStore,
    SshTarget, TaskArtifactStore, TaskDispatch, TaskInputBinding, TaskInputSource,
    TaskOutputBinding, TaskResources, TaskSpec, TaskTransport,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const PAYLOAD: &str = r#"import hashlib, json, os, pathlib
source = pathlib.Path(os.environ['AUTONOMICS_INPUT_F_0'])
raw = source.read_bytes()
assert json.loads(raw) == [1, 2, 3]
assert os.environ['DECLARED'] == "literal $HOME $(touch SHOULD_NOT_EXIST)"
assert os.getuid() == source.stat().st_uid
assert pathlib.Path(os.environ['HOME']).is_relative_to(pathlib.Path.cwd())
tmp = pathlib.Path(os.environ['TMPDIR'])
assert tmp.is_relative_to(pathlib.Path.cwd())
(tmp / 'synthetic.tmp').write_text('temporary output')
cgroup = pathlib.Path('/proc/self/cgroup').read_text()
result = {'sum_of_squares': sum(x*x for x in json.loads(raw)),
          'input_sha256': hashlib.sha256(raw).hexdigest(), 'uid': os.getuid(),
          'cwd': str(pathlib.Path.cwd()), 'home': os.environ['HOME'],
          'tmpdir': str(tmp), 'cgroup': cgroup}
pathlib.Path('result.json').write_text(json.dumps(result, sort_keys=True))
assert not pathlib.Path('SHOULD_NOT_EXIST').exists()
"#;

fn retain(root: &Path, name: &str, value: &Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}

async fn observation(ssh: &SshTarget, command: &str) -> Result<Value> {
    let output = ssh.run(command).await?;
    Ok(json!({"exit_code": output.status.code(),
        "stdout": String::from_utf8(output.stdout)?, "stderr": String::from_utf8(output.stderr)?}))
}

async fn stop_proof(ssh: &SshTarget, job: &str) -> Result<Value> {
    if job.is_empty() || !job.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid job id".into());
    }
    let mut last = Value::Null;
    for _ in 0..15 {
        let queue = observation(ssh, &format!("squeue -h -j {job} -o %T")).await?;
        // No -X: retain main, batch and extern termination independently.
        let accounting = observation(
            ssh,
            &format!("sacct -n -P -j {job} --format=JobID,State%40,ExitCode,End"),
        )
        .await?;
        let details = observation(ssh, &format!("scontrol show job -o {job}")).await?;
        let rows: Vec<Vec<&str>> = accounting["stdout"]
            .as_str()
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.split('|').collect())
            .collect();
        let good_rows = rows.len() >= 3
            && [
                job.to_string(),
                format!("{job}.batch"),
                format!("{job}.extern"),
            ]
            .iter()
            .all(|id| {
                rows.iter()
                    .filter(|row| row.first() == Some(&id.as_str()))
                    .count()
                    == 1
            })
            && rows.iter().all(|row| {
                row.len() == 4
                    && (row[0] == job || row[0].starts_with(&format!("{job}.")))
                    && matches!(
                        row[1],
                        "COMPLETED"
                            | "FAILED"
                            | "CANCELLED"
                            | "TIMEOUT"
                            | "OUT_OF_MEMORY"
                            | "NODE_FAIL"
                            | "PREEMPTED"
                    )
                    && !matches!(row[3], "" | "Unknown" | "None")
            });
        let control = details["stdout"].as_str().unwrap();
        let control_matches = rows
            .iter()
            .find(|row| row.first() == Some(&job))
            .is_some_and(|row| {
                row.len() == 4
                    && [
                        format!("JobId={job}"),
                        format!("JobState={}", row[1]),
                        format!("ExitCode={}", row[2]),
                    ]
                    .iter()
                    .all(|field| control.split_whitespace().any(|word| word == field))
            });
        let proven = queue["exit_code"] == 0
            && queue["stdout"].as_str().unwrap().trim().is_empty()
            && accounting["exit_code"] == 0
            && details["exit_code"] == 0
            && good_rows
            && control_matches;
        last = json!({"job_id": job, "queue": queue, "accounting": accounting,
            "details": details, "full_subtree_terminal": proven, "remote_scratch_cleaned": false});
        if proven {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(last)
}

// Record the complete split backend as one deterministic source artifact.
const SLURM_SOURCE: &str = concat!(
    include_str!("../src/dag/execution/slurm/mod.rs"),
    include_str!("../src/dag/execution/slurm/artifact.rs"),
    include_str!("../src/dag/execution/slurm/routing.rs"),
    include_str!("../src/dag/execution/slurm/settings.rs"),
    include_str!("../src/dag/execution/slurm/ssh.rs"),
    include_str!("../src/dag/execution/slurm/transport.rs"),
    include_str!("../src/dag/execution/slurm/slurm_submission.rs"),
    include_str!("../src/dag/execution/slurm/read_heartbeat.py"),
    include_str!("../src/dag/execution/slurm/slurm_logs.py"),
    include_str!("../src/dag/execution/slurm/slurm_submission_observer.py"),
);

pub async fn run(manifest: &Path, image: &str, permanent: &Path) -> Result<()> {
    let settings = ApptainerSettings::load(manifest)?;
    if !settings.images.contains_key(image) {
        return Err("image is not in manifest".into());
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let task = format!("apptainer-smoke-{nonce}");
    let scratch = format!("~/autonomics-acceptance/{task}");
    let ssh = SshTarget::new("ssh", "sjtu");
    let root = permanent.join("apptainer-smoke");
    std::fs::create_dir(&root)?;
    retain(
        &root,
        "START.json",
        &json!({"schema": "APPTAINER_SLURM_SYNTHETIC_SMOKE_V1",
        "task_id": task, "remote_scratch": scratch, "image": image, "settings": settings,
        "g0_granted": false, "scientific_execution": false,
        "renderer_sha256": format!("{:x}", Sha256::digest(include_bytes!("../src/dag/execution/apptainer.rs"))),
        "process_sha256": format!("{:x}", Sha256::digest(include_bytes!("../src/dag/execution/process.rs"))),
        "smoke_sha256": format!("{:x}", Sha256::digest(include_bytes!("apptainer_slurm_smoke.rs"))),
        "transport_sha256": format!("{:x}", Sha256::digest(SLURM_SOURCE.as_bytes()))}),
    )?;
    // Preserve exact new source bytes independently of the mutable checkout.
    std::fs::write(
        root.join("apptainer.rs"),
        include_bytes!("../src/dag/execution/apptainer.rs"),
    )?;
    std::fs::write(root.join("slurm.rs"), SLURM_SOURCE.as_bytes())?;
    std::fs::write(
        root.join("process.rs"),
        include_bytes!("../src/dag/execution/process.rs"),
    )?;
    std::fs::write(
        root.join("apptainer_slurm_smoke.rs"),
        include_bytes!("apptainer_slurm_smoke.rs"),
    )?;
    let source = root.join("synthetic-input.json");
    std::fs::write(&source, "[1,2,3]")?;
    let store = SshArtifactStore::new(ssh.clone(), &scratch, root.join("downloads"));
    let uploaded = store
        .upload(
            FileRef::local(&source, Some("json".into()))?,
            &task,
            "input-0-0",
        )
        .await?;
    let process = ProcessTaskSpec {
        container: Some(ApptainerContainer {
            image: image.into(),
            mode: ApptainerMode::Exec,
            network: ApptainerNetwork::Host,
        }),
        script: None,
        command: vec!["python3".into(), "-c".into(), PAYLOAD.into()],
        env: BTreeMap::from([(
            "DECLARED".into(),
            "literal $HOME $(touch SHOULD_NOT_EXIST)".into(),
        )]),
        outputs: vec![ProcessTaskOutput {
            port: 0,
            pattern: "result.json".into(),
            optional: false,
        }],
    };
    let dispatch = TaskDispatch {
        spec: TaskSpec {
            id: task.clone(),
            logical_node: None,
            kind: "process".into(),
            spec: serde_json::to_value(process)?,
            item_key: None,
            item: None,
            inputs: vec![TaskInputBinding {
                name: "f".into(),
                port: 0,
                source: TaskInputSource::UpstreamPort {
                    from: "synthetic".into(),
                    from_port: 0,
                },
                payload: "file".into(),
                path: Some(source.to_string_lossy().into_owned()),
                fingerprint: None,
                staged_paths: vec![uploaded.path.clone()],
            }],
            outputs: vec![TaskOutputBinding {
                name: "result".into(),
                port: 0,
                payload: "file".into(),
                artifact_paths: vec![],
            }],
        },
        resources: TaskResources {
            cpus: Some(1),
            memory_bytes: Some(2 * 1024 * 1024 * 1024),
            max_duration_ms: Some(120_000),
        },
    };
    retain(&root, "DISPATCH.json", &serde_json::to_value(&dispatch)?)?;
    let transport = SlurmTaskTransport::new(SlurmSettings {
        apptainer: Some(settings),
        ssh: ssh.clone(),
        scratch_dir: scratch,
        local_artifact_root: root.clone(),
        partition: "debug64c512g".into(),
        account: None,
        gpus: None,
        qos: None,
        mem_per_cpu: "2G".into(),
        default_time: "00:02:00".into(),
        poll_interval: Duration::from_secs(2),
    });
    let lease = transport.submit(dispatch, vec![uploaded]).await?;
    retain(
        &root,
        "LEASE.json",
        &json!({"job_id": lease.lease_id, "task_id": task}),
    )?;
    let result = tokio::time::timeout(Duration::from_secs(300), transport.wait(&lease)).await;
    let wait = match result {
        Ok(Ok(receipt)) => {
            retain(&root, "RECEIPT.json", &serde_json::to_value(&receipt)?)?;
            Ok(receipt)
        }
        _ => {
            let cancel = transport.cancel(&lease).await;
            retain(
                &root,
                "WAIT_FAILURE.json",
                &json!({"cancel_confirmed_by_transport": cancel.is_ok()}),
            )?;
            Err("remote wait failed or exceeded observer deadline")
        }
    };
    let proof = stop_proof(&ssh, &lease.lease_id).await?;
    retain(&root, "STOP_PROOF.json", &proof)?;
    if proof["full_subtree_terminal"] != true {
        return Err("remote stop UNKNOWN; scratch retained".into());
    }
    let receipt = wait?;
    let mut diagnostics = BTreeMap::new();
    for name in [
        "job.sbatch".to_string(),
        "task.json".into(),
        format!("slurm-{}.out", lease.lease_id),
        format!("slurm-{}.err", lease.lease_id),
    ] {
        let path = format!("{}/{}", receipt.workspace, name);
        let quoted = format!("'{}'", path.replace('\'', "'\\''"));
        diagnostics.insert(name, observation(&ssh, &format!("cat -- {quoted}")).await?);
    }
    retain(&root, "JOB_FILES.json", &serde_json::to_value(diagnostics)?)?;
    if receipt.exit_code != 0 {
        return Err("synthetic Apptainer job failed; retained receipt and remote scratch".into());
    }
    let artifact = receipt
        .output_artifacts_by_port
        .get(&0)
        .and_then(|files| files.first())
        .ok_or("missing output")?;
    let downloaded = store
        .download(artifact.clone(), &task, "result.json")
        .await?;
    let output: Value = serde_json::from_slice(&std::fs::read(&downloaded.path)?)?;
    let input_hash = format!("{:x}", Sha256::digest(b"[1,2,3]"));
    if output["sum_of_squares"] != 14 || output["input_sha256"] != input_hash {
        return Err("synthetic output differs".into());
    }
    let group = output["cgroup"]
        .as_str()
        .ok_or("missing cgroup observation")?;
    if !group.contains(&format!("job_{}", lease.lease_id))
        && !group.contains(&format!("job-{}", lease.lease_id))
    {
        return Err("payload did not report its recorded Slurm job cgroup".into());
    }
    retain(
        &root,
        "RESULT.json",
        &json!({"status": "SYNTHETIC_SMOKE_PASS", "job_id": lease.lease_id,
        "input_sha256": input_hash, "full_subtree_terminal": true,
        "remote_scratch_cleaned": false, "scientific_qualification": false, "g0_granted": false}),
    )?;
    println!(
        "SYNTHETIC_SMOKE_PASS job={} (remote scratch retained)",
        lease.lease_id
    );
    Ok(())
}

#[allow(dead_code)] // Also included by the explicitly ignored site test.
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err(
            "usage: apptainer_slurm_smoke <operator-manifest.json> <pinned-python-image-reference>"
                .into(),
        );
    }
    let permanent = std::env::var("AUTONOMICS_VALIDATION_PERMANENT")?;
    run(Path::new(&args[1]), &args[2], Path::new(&permanent)).await
}
