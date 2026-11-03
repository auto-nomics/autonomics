//! Operator-reviewed environment diagnostics are separate from an old attempt.
//! Only the host operator can install these private records outside agent VFS.
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::HpcClient;
use crate::jobs::{REMOTE_JOBS_HELPER, digest, local_record_path, read_local_record};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ssh_host: String,
    submission_key: String,
    nonce: String,
    request_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    review_sha256: String,
    result_sha256: String,
    status_sha256: String,
    read_only: bool,
    output_isolated: bool,
    environment_identity_verified: bool,
    scientific_processing_authorized: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Followup {
    version: u32,
    kind: String,
    original: Binding,
    successor: Binding,
    successor_job_id: String,
    approval: Approval,
}

fn bound(binding: &Binding, record: &Value, client: &HpcClient) -> bool {
    binding.ssh_host == client.config().ssh_host
        && record["version"] == 1
        && record["ssh_host"] == binding.ssh_host
        && record["submission_key"] == binding.submission_key
        && record["nonce"] == binding.nonce
        && record["request_sha256"] == binding.request_sha256
        && record["request"]["action"] == "submit"
        && serde_json::to_vec(&record["request"])
            .is_ok_and(|bytes| digest(&bytes) == binding.request_sha256)
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

async fn query(client: &HpcClient, input: Value) -> Result<Value, String> {
    let response = client.run_json(REMOTE_JOBS_HELPER, &input, false).await?;
    if response["ok"] != true {
        return Err("Cannot verify the operator-reviewed followup against remote records".into());
    }
    Ok(response)
}

pub(super) async fn resolve(
    client: &HpcClient,
    original_record: &Value,
    original_response: &Value,
) -> Result<Option<Value>, String> {
    if original_response["state"] != "UNKNOWN" {
        return Ok(None);
    }
    let directory = client.state_dir().join("operator-followups");
    let info = match fs::symlink_metadata(&directory) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot inspect operator followup directory".into()),
    };
    if !info.is_dir()
        || info.uid() != unsafe { libc::geteuid() }
        || info.permissions().mode() & 0o7777 != 0o700
    {
        return Err("Operator followup directory must be private, owned and nonsymlink".into());
    }
    let nonce = original_record["nonce"]
        .as_str()
        .ok_or("Original nonce is absent")?;
    if nonce.len() != 32
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Original nonce is invalid".into());
    }
    let Some(value) = read_local_record(&directory.join(format!("{nonce}.json")))? else {
        return Ok(None);
    };
    let followup: Followup =
        serde_json::from_value(value).map_err(|_| "Invalid operator followup record")?;
    let approval = &followup.approval;
    if followup.version != 1
        || followup.kind != "reviewed_readonly_environment_diagnostic"
        || !approval.read_only
        || !approval.output_isolated
        || !approval.environment_identity_verified
        || approval.scientific_processing_authorized
        || ![
            &approval.review_sha256,
            &approval.result_sha256,
            &approval.status_sha256,
        ]
        .into_iter()
        .all(|v| sha256(v))
        || !bound(&followup.original, original_record, client)
        || followup.successor.nonce == nonce
        || followup.successor.nonce.len() != 32
        || !followup
            .successor
            .nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || followup.successor.submission_key == followup.original.submission_key
        || followup.successor_job_id.is_empty()
        || followup.successor_job_id.len() > 12
        || followup.successor_job_id.starts_with('0')
        || !followup
            .successor_job_id
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        return Err("Operator followup scope or original identity does not match".into());
    }
    let target_path = local_record_path(client, &followup.successor.submission_key)?;
    let target =
        read_local_record(&target_path)?.ok_or("Operator followup local intent is missing")?;
    if !bound(&followup.successor, &target, client) {
        return Err("Operator followup successor identity does not match".into());
    }
    // Verify the remote frozen specification, script bytes and accounting.
    // Never call submit or run a diagnostic script during this lookup.
    let remote = query(
        client,
        json!({"tool":"jobs", "action":"verify_operator_followup",
        "nonce":followup.successor.nonce, "expected_request":target["request"],
        "expected_job_id":followup.successor_job_id}),
    )
    .await?;
    if remote["remote_intent_verified"] != true
        || remote["nonce"] != followup.successor.nonce
        || remote["job_id"] != followup.successor_job_id
        || remote["script_sha256"]
            != digest(
                target["request"]["script"]
                    .as_str()
                    .ok_or("Followup script missing")?
                    .as_bytes(),
            )
    {
        return Err("Operator followup remote intent does not match the reviewed job".into());
    }
    let status = remote["status"].clone();
    let complete = status["job_id"] == followup.successor_job_id
        && status["ownership_verified"] == true
        && status["stop_confirmed"] == true
        && status["successful"] == true;
    Ok(Some(json!({
        "ok":true,
        "state":if complete {"OPERATOR_FOLLOWUP_COMPLETED"} else {"OPERATOR_FOLLOWUP_UNVERIFIED"},
        "original_submission":original_response,
        "original_outcome_resolved":false,
        "submission_retried":false,
        "new_job_submitted":false,
        "scientific_admission_granted":false,
        "followup":{
            "kind":"reviewed_readonly_environment_diagnostic",
            "submission_key":followup.successor.submission_key,
            "nonce":followup.successor.nonce,
            "job_id":followup.successor_job_id,
            "ssh_host":followup.successor.ssh_host,
            "scope":"environment_identity_only",
            "operator_review_sha256":approval.review_sha256,
            "status":status
        },
        "next_action":if complete {
            "Use the completed operator-reviewed followup for the environment preflight. Do not recover or submit this old key again: its historical UNKNOWN is retained, not a global workflow lock. Verify remaining frozen inputs, resources and scientific approvals before processing."
        } else {
            "Inspect the reviewed followup job and operator evidence. Do not retry this old key or claim preflight completion while followup status is unverified."
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolRegistration;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::Arc;

    struct Fixture {
        _directory: tempfile::TempDir,
        client: Arc<HpcClient>,
        old: Value,
        receipt: Value,
        path: std::path::PathBuf,
        flags: std::path::PathBuf,
        calls: std::path::PathBuf,
    }

    fn private(path: &std::path::Path, value: &Value) {
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn record(client: &HpcClient, key: &str, nonce: &str, working: &str) -> Value {
        let request = json!({"tool":"jobs","action":"submit","submission_key":key,
            "script":"#!/bin/sh\ntrue\n","partition":"cpu","working_directory":working});
        let record = json!({"version":1,"ssh_host":"toy","submission_key":key,
            "nonce":nonce,"request_sha256":digest(&serde_json::to_vec(&request).unwrap()),"request":request});
        private(&local_record_path(client, key).unwrap(), &record);
        record
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let ssh = directory.path().join("fake-ssh");
            let flags = directory.path().join("flags.json");
            let calls = directory.path().join("calls.jsonl");
            private(&flags, &json!({}));
            let script = format!(
                r#"#!/usr/bin/env python3
import json,sys,hashlib
flags=json.load(open({flags}))
i=json.load(sys.stdin)['input']
with open({calls},'a') as f:f.write(json.dumps(i)+'\n')
if i['action']=='validate_submission_request':
 print(json.dumps({{'ok':True,'state':'VALIDATED','submission_attempted':False}}))
elif i['action'] in ('recover','submit'):
 assert i['nonce']=='a'*32, 'never submit or recover a successor blindly'
 print(json.dumps({{'ok':True,'state':'SUBMITTED' if flags.get('old_submitted') else 'UNKNOWN','nonce':i['nonce'],'reason':'no_exact_match','matches':[],'submission_retried':False}}))
elif i['action']=='verify_operator_followup':
 assert i['nonce']=='b'*32 and i['expected_job_id']=='123'
 assert i['expected_request']['working_directory']=='/tmp/isolated'
 print(json.dumps({{'ok':True,'nonce':i['nonce'],'job_id':'123','remote_intent_verified':True,
 'script_sha256':'0'*64 if flags.get('script_mismatch') else hashlib.sha256(i['expected_request']['script'].encode()).hexdigest(),
 'status':{{'job_id':'123','ownership_verified':True,'stop_confirmed':not flags.get('active'),'successful':not flags.get('active')}}}}))
else:raise AssertionError('unexpected operation')
"#,
                flags = serde_json::to_string(&flags).unwrap(),
                calls = serde_json::to_string(&calls).unwrap()
            );
            fs::write(&ssh, script).unwrap();
            fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
            let client = Arc::new(
                HpcClient::new(crate::HpcConfig {
                    ssh_host: "toy".into(),
                    data_ssh_host: None,
                    pi_ssh_host: None,
                    ssh_command: ssh.to_string_lossy().into_owned(),
                    remote_python: "python3".into(),
                    state_dir: directory.path().join("state"),
                    default_partition: "cpu".into(),
                    default_account: None,
                })
                .unwrap(),
            );
            let old = record(&client, "old", &"a".repeat(32), "/tmp/original");
            let next = record(&client, "diagnostic", &"b".repeat(32), "/tmp/isolated");
            let binding = |r: &Value| json!({"ssh_host":r["ssh_host"],"submission_key":r["submission_key"],"nonce":r["nonce"],"request_sha256":r["request_sha256"]});
            let receipt = json!({"version":1,"kind":"reviewed_readonly_environment_diagnostic",
                "original":binding(&old),"successor":binding(&next),"successor_job_id":"123",
                "approval":{"review_sha256":"c".repeat(64),"result_sha256":"d".repeat(64),"status_sha256":"e".repeat(64),
                "read_only":true,"output_isolated":true,"environment_identity_verified":true,"scientific_processing_authorized":false}});
            let dir = client.state_dir().join("operator-followups");
            fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
            let path = dir.join(format!("{}.json", "a".repeat(32)));
            private(&path, &receipt);
            Self {
                _directory: directory,
                client,
                old,
                receipt,
                path,
                flags,
                calls,
            }
        }

        fn tool(&self) -> ToolRegistration {
            crate::jobs::registrations(self.client.clone())
                .into_iter()
                .find(|t| t.definition.name == "hpc_jobs")
                .unwrap()
        }

        async fn call(&self, action: &str) -> Result<Value, agentik_core::tools::ToolError> {
            let input = if action == "submit" {
                let mut input = self.old["request"].clone();
                input.as_object_mut().unwrap().remove("tool");
                input
            } else {
                json!({"action":action,"submission_key":"old"})
            };
            let result = self.tool().implementation.execute(input).await?;
            Ok(serde_json::to_value(result).unwrap()["content"].clone())
        }

        fn calls(&self) -> Vec<Value> {
            fs::read_to_string(&self.calls)
                .unwrap_or_default()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        }
    }

    /// Explicit operator acceptance only; default test runs never use real SSH.
    #[tokio::test]
    #[ignore = "requires private operator configuration and managed permanent evidence"]
    async fn operator_live_recover_probe() {
        let path = std::env::var("AUTONOMICS_HPC_OPERATOR_PROBE_CONFIG").unwrap();
        let input = read_local_record(std::path::Path::new(&path))
            .unwrap()
            .unwrap();
        let text = |name: &str| input[name].as_str().unwrap().to_owned();
        let client = Arc::new(
            HpcClient::new(crate::HpcConfig {
                ssh_host: text("ssh_host"),
                data_ssh_host: None,
                pi_ssh_host: None,
                ssh_command: text("ssh_command"),
                remote_python: text("remote_python"),
                state_dir: text("state_dir").into(),
                default_partition: text("default_partition"),
                default_account: input["default_account"].as_str().map(str::to_owned),
            })
            .unwrap(),
        );
        let tool = crate::jobs::registrations(client)
            .into_iter()
            .find(|t| t.definition.name == "hpc_jobs")
            .unwrap();
        let result = tool
            .implementation
            .execute(json!({"action":"recover",
            "submission_key":text("submission_key")}))
            .await
            .unwrap();
        let value = serde_json::to_value(result).unwrap()["content"].clone();
        let root =
            std::path::PathBuf::from(std::env::var("AUTONOMICS_VALIDATION_PERMANENT").unwrap());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("LIVE_HPC_JOBS_RECOVER.json"))
            .unwrap();
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        assert_eq!(value["state"], input["expected_state"]);
        assert_eq!(
            value["followup"]["job_id"],
            input["expected_followup_job_id"]
        );
        assert_eq!(
            value["original_submission"]["nonce"],
            input["expected_original_nonce"]
        );
        assert_eq!(value["original_submission"]["state"], "UNKNOWN");
        assert_eq!(value["new_job_submitted"], false);
        assert_eq!(value["scientific_admission_granted"], false);
    }

    #[tokio::test]
    async fn registered_recover_and_same_key_submit_return_the_completed_operator_followup() {
        let f = Fixture::new();
        for action in ["recover", "submit"] {
            let result = f.call(action).await.unwrap();
            assert_eq!(result["state"], "OPERATOR_FOLLOWUP_COMPLETED");
            assert_eq!(result["original_submission"]["state"], "UNKNOWN");
            assert_eq!(result["followup"]["job_id"], "123");
            assert_eq!(result["followup"]["scope"], "environment_identity_only");
            assert_eq!(result["original_outcome_resolved"], false);
            assert_eq!(result["scientific_admission_granted"], false);
            assert_eq!(result["new_job_submitted"], false);
        }
        let calls = f.calls();
        assert_eq!(calls.len(), 5);
        assert_eq!(
            calls
                .iter()
                .filter(|c| c["action"] == "verify_operator_followup")
                .count(),
            2
        );
        assert!(
            !calls
                .iter()
                .any(|c| c["action"] == "submit" && c["nonce"] != "a".repeat(32))
        );
        assert_eq!(
            read_local_record(&local_record_path(&f.client, "old").unwrap())
                .unwrap()
                .unwrap(),
            f.old
        );
    }

    #[tokio::test]
    async fn ordinary_unknown_without_operator_receipt_is_unchanged() {
        let f = Fixture::new();
        fs::remove_file(&f.path).unwrap();
        assert_eq!(f.call("recover").await.unwrap()["state"], "UNKNOWN");
        assert_eq!(f.calls().len(), 1);
    }

    #[tokio::test]
    async fn unsafe_or_rebound_receipts_fail_closed_before_successor_lookup() {
        for field in ["nonce", "request_sha256", "ssh_host", "submission_key"] {
            let mut f = Fixture::new();
            f.receipt["original"][field] = json!("wrong");
            private(&f.path, &f.receipt);
            assert!(f.call("recover").await.is_err());
            assert_eq!(f.calls().len(), 1);
        }
        let f = Fixture::new();
        fs::set_permissions(&f.path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(f.call("recover").await.is_err());
        assert_eq!(f.calls().len(), 1);
    }

    #[tokio::test]
    async fn scientific_or_unreviewed_scope_cannot_be_promoted() {
        for field in [
            "read_only",
            "output_isolated",
            "environment_identity_verified",
            "scientific_processing_authorized",
        ] {
            let mut f = Fixture::new();
            f.receipt["approval"][field] = json!(field == "scientific_processing_authorized");
            private(&f.path, &f.receipt);
            assert!(f.call("recover").await.is_err());
            assert_eq!(f.calls().len(), 1);
        }
    }

    #[tokio::test]
    async fn changed_successor_specification_or_endpoint_is_rejected() {
        for field in ["request_sha256", "ssh_host", "nonce"] {
            let mut f = Fixture::new();
            f.receipt["successor"][field] = json!("wrong");
            private(&f.path, &f.receipt);
            assert!(f.call("recover").await.is_err());
            assert_eq!(f.calls().len(), 1);
        }
    }

    #[tokio::test]
    async fn mismatched_remote_script_cannot_reuse_completed_job_identity() {
        let f = Fixture::new();
        private(&f.flags, &json!({"script_mismatch":true}));
        assert!(f.call("recover").await.is_err());
        assert_eq!(f.calls().len(), 2);
    }

    #[tokio::test]
    async fn unfinished_followup_is_explicit_and_never_claimed_complete() {
        let f = Fixture::new();
        private(&f.flags, &json!({"active":true}));
        let result = f.call("recover").await.unwrap();
        assert_eq!(result["state"], "OPERATOR_FOLLOWUP_UNVERIFIED");
        assert_eq!(result["original_submission"]["state"], "UNKNOWN");
        assert_eq!(result["new_job_submitted"], false);
    }

    #[tokio::test]
    async fn known_original_submission_is_never_relabelled_as_an_independent_job() {
        let f = Fixture::new();
        private(&f.flags, &json!({"old_submitted":true}));
        assert_eq!(f.call("recover").await.unwrap()["state"], "SUBMITTED");
        assert_eq!(f.calls().len(), 1);
    }

    #[tokio::test]
    async fn agent_input_cannot_install_or_inject_operator_followup_approval() {
        let f = Fixture::new();
        let result = f
            .tool()
            .implementation
            .execute(
                json!({"action":"recover","submission_key":"old","operator_followup":f.receipt}),
            )
            .await;
        assert!(result.is_err());
        assert!(f.calls().is_empty());
    }
}
