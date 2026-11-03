//! Strict projection of actual container-worker measurements. Configuration
//! identities and coordinator binaries are never substituted for worker code.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoftwareFile {
    path: String,
    sha256: String,
    bytes: u64,
}

impl SoftwareFile {
    fn valid(&self) -> bool {
        self.path.starts_with('/')
            && !self.path.chars().any(char::is_control)
            && self.bytes > 0
            && digest(&self.sha256)
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerSoftware {
    schema: String,
    worker_kind: String,
    task_id: String,
    job_id: String,
    manifest_sha256: String,
    request_sha256: String,
    worker_script: SoftwareFile,
    interpreter: SoftwareFile,
    container_runtime: SoftwareFile,
    sif: Option<SoftwareFile>,
}

pub(crate) struct ExpectedWorkerSoftware<'a> {
    pub task_id: &'a str,
    pub job_id: &'a str,
    pub manifest_sha256: &'a str,
    pub request_sha256: &'a str,
    pub worker_sha256: &'a str,
    pub sif_path: &'a str,
    pub sif_sha256: &'a str,
    pub succeeded: bool,
}

pub(crate) fn verify(value: &Value, expected: ExpectedWorkerSoftware<'_>) -> Result<Value, &'static str> {
    let evidence: WorkerSoftware = serde_json::from_value(value.clone())
        .map_err(|_| "invalid worker software evidence schema")?;
    if evidence.schema != "APPTAINER_WORKER_SOFTWARE_V1"
        || evidence.worker_kind != "python-apptainer"
        || evidence.task_id != expected.task_id
        || evidence.job_id != expected.job_id
        || !digest(&evidence.manifest_sha256)
        || evidence.manifest_sha256 != expected.manifest_sha256
        || !digest(&evidence.request_sha256)
        || evidence.request_sha256 != expected.request_sha256
        || !evidence.worker_script.valid()
        || evidence.worker_script.sha256 != expected.worker_sha256
        || !evidence.interpreter.valid()
        || !evidence.container_runtime.valid()
    {
        return Err("worker software evidence does not match the submitted attempt");
    }
    match &evidence.sif {
        Some(sif) if sif.valid() && sif.path == expected.sif_path && sif.sha256 == expected.sif_sha256 => {}
        None if !expected.succeeded => {}
        _ => return Err("worker software evidence has no matching measured SIF"),
    }
    serde_json::to_value(evidence).map_err(|_| "cannot encode worker software evidence")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn packet() -> Value {
        let file = |name: &str, hash: char| json!({"path":format!("/software/{name}"),
            "sha256":hash.to_string().repeat(64),"bytes":123});
        json!({"schema":"APPTAINER_WORKER_SOFTWARE_V1","worker_kind":"python-apptainer",
            "task_id":"attempt-one","job_id":"123","manifest_sha256":"a".repeat(64),
            "request_sha256":"b".repeat(64),"worker_script":file("worker.py",'c'),
            "interpreter":file("python",'d'),"container_runtime":file("apptainer",'e'),
            "sif":file("image.sif",'f')})
    }

    fn verify_packet(value: &Value, succeeded: bool) -> Result<Value, &'static str> {
        verify(value, ExpectedWorkerSoftware {task_id:"attempt-one", job_id:"123",
            manifest_sha256:&"a".repeat(64),request_sha256:&"b".repeat(64),
            worker_sha256:&"c".repeat(64),sif_path:"/software/image.sif",
            sif_sha256:&"f".repeat(64),succeeded})
    }

    #[test]
    fn accepts_exact_measurements_and_rejects_other_attempts_or_software() {
        assert_eq!(verify_packet(&packet(), true).unwrap(), packet());
        for field in ["task_id", "job_id", "manifest_sha256", "request_sha256", "worker_kind"] {
            let mut changed = packet();
            changed[field] = "foreign".into();
            assert!(verify_packet(&changed, true).is_err(), "{field}");
        }
        for field in ["worker_script", "interpreter", "container_runtime", "sif"] {
            let mut changed = packet();
            changed[field]["sha256"] = "invalid".into();
            assert!(verify_packet(&changed, true).is_err(), "{field}");
        }
        let mut changed = packet();
        changed["sif"]["sha256"] = "0".repeat(64).into();
        assert!(verify_packet(&changed, true).is_err());
        changed = packet();
        changed["worker_script"]["sha256"] = "0".repeat(64).into();
        assert!(verify_packet(&changed, true).is_err());
    }

    #[test]
    fn missing_image_measurement_is_only_valid_for_failed_work_and_unknown_fields_are_refused() {
        let mut value = packet();
        value["sif"] = Value::Null;
        assert!(verify_packet(&value, false).is_ok());
        assert!(verify_packet(&value, true).is_err());
        value["environment"] = json!({"PRIVATE":"must not enter the audit"});
        assert!(verify_packet(&value, false).is_err());
        value = packet();
        value["interpreter"]["environment"] = json!({});
        assert!(verify_packet(&value, true).is_err());
    }
}
