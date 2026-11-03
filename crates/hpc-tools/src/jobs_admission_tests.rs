//! Registered-tool admission with the real remote parser and a toy scheduler.
//! The fake SSH child is joined by the transport; no cluster access is used.
use super::*;

struct Fixture {
    root: tempfile::TempDir,
    tool: JobsTool,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("remote");
        fs::create_dir(&home).unwrap();
        let ssh = root.path().join("ssh");
        fs::write(&ssh, r#"#!/usr/bin/env python3
import glob,json,os,sys,types
root=os.path.dirname(os.path.abspath(__file__))
envelope=json.load(sys.stdin)
request=envelope['input']
with open(os.path.join(root,'requests.jsonl'),'a') as f:
    f.write(json.dumps(request)+'\n')
if request['action']=='validate_submission_request':
    frozen_before={p:open(p,'rb').read() for p in glob.glob(os.path.join(root,'state','job-intents','*.json'))}
if request['action']=='submit':
    assert glob.glob(os.path.join(root,'state','job-intents','*.json'))
namespace={}
exec(envelope['code'],namespace)
namespace['pwd']=types.SimpleNamespace(getpwuid=lambda uid:types.SimpleNamespace(pw_dir=os.path.join(root,'remote'),pw_name='toy'))
def command(argv,timeout=20,required=True):
    assert argv[0]=='sbatch'
    with open(os.path.join(root,'scheduler.jsonl'),'a') as f:
        f.write(json.dumps(argv)+'\n')
    return {'available':True,'returncode':0,'stdout':'' if '--test-only' in argv else '123\n'}
namespace['command']=command
result=namespace['run'](request)
if request['action']=='validate_submission_request':
    assert frozen_before=={p:open(p,'rb').read() for p in glob.glob(os.path.join(root,'state','job-intents','*.json'))}
print(json.dumps(result))
"#).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let client = Arc::new(
            HpcClient::new(crate::HpcConfig {
                ssh_host: "toy".into(),
                data_ssh_host: None,
                pi_ssh_host: None,
                ssh_command: ssh.to_string_lossy().into_owned(),
                remote_python: "python3".into(),
                state_dir: root.path().join("state"),
                default_partition: "cpu".into(),
                default_account: None,
            })
            .unwrap(),
        );
        Self {
            root,
            tool: JobsTool {
                client,
                monitoring: None,
            },
            home,
        }
    }

    fn request(&self) -> Value {
        json!({"action":"submit","submission_key":"repairable",
            "script":"#!/bin/sh\ntrue\n","working_directory":self.home})
    }

    async fn invoke(&self, request: Value) -> Result<ToolResult, ToolError> {
        self.tool
            .run(serde_json::from_value(request).unwrap())
            .await
    }

    fn calls(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.root.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test]
async fn malformed_new_submit_can_be_corrected_under_the_same_key() {
    let f = Fixture::new();
    let path = local_record_path(&f.tool.client, "repairable").unwrap();
    let mut request = f.request();
    request["array"] = json!("0;invalid");
    assert_eq!(f.invoke(request.clone()).await.unwrap().is_error, Some(true));
    assert!(!path.exists());
    assert!(!f.home.join(".autonomics-hpc-jobs").exists());
    assert!(f.calls("scheduler.jsonl").is_empty());

    request["array"] = Value::Null;
    let result = f.invoke(request.clone()).await.unwrap();
    let result: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(result["state"], "SUBMITTED");
    let frozen = fs::read(&path).unwrap();
    let nonce = serde_json::from_slice::<Value>(&frozen).unwrap()["nonce"].clone();
    // The same valid submit adopts the durable receipt instead of submitting again.
    let again = f.invoke(request).await.unwrap();
    let again: Value = serde_json::from_str(&again.text_content()).unwrap();
    assert_eq!(again["nonce"], nonce);
    assert_eq!(fs::read(path).unwrap(), frozen);
    assert_eq!(f.calls("scheduler.jsonl").len(), 2); // test-only + one real submit
    assert_eq!(
        f.calls("requests.jsonl")
            .iter()
            .filter(|r| r["action"] == "validate_submission_request")
            .count(),
        3
    );
}

#[tokio::test]
async fn quoted_null_and_empty_options_are_rejected_before_key_binding_or_ssh() {
    let f = Fixture::new();
    for field in [
        "array",
        "dependency",
        "qos",
        "account",
        "constraint",
        "reservation",
    ] {
        for value in ["null", ""] {
            let mut request = f.request();
            request[field] = json!(value);
            let error = f.invoke(request).await.err().unwrap().to_string();
            assert!(error.contains(field), "{error}");
            assert!(error.contains("JSON null"), "{error}");
        }
    }
    assert!(f.calls("requests.jsonl").is_empty());
    assert!(
        !local_record_path(&f.tool.client, "repairable")
            .unwrap()
            .exists()
    );
}

#[tokio::test]
async fn legacy_missing_intent_stays_unknown_and_changed_recovery_script_is_rejected() {
    let f = Fixture::new();
    let mut frozen = f.request();
    frozen["array"] = json!("null"); // historical malformed request: preserve its bytes
    let record = intent(&f.tool.client, "repairable", Some(&frozen)).unwrap();
    let path = local_record_path(&f.tool.client, "repairable").unwrap();
    let before = fs::read(&path).unwrap();
    for (field, changed) in [
        ("script", json!("#!/bin/sh\nfalse\n")),
        ("memory_mb", json!(8192)),
    ] {
        let mut recovery = json!({"action":"recover","submission_key":"repairable"});
        recovery[field] = changed;
        assert!(
            f.invoke(recovery)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains(field)
        );
    }
    assert!(f.calls("requests.jsonl").is_empty());
    let result = f
        .invoke(json!({"action":"recover","submission_key":"repairable"}))
        .await
        .unwrap();
    let result: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(result["state"], "UNKNOWN");
    assert_eq!(result["nonce"], record["nonce"]);
    assert_eq!(result["reason"], "remote_intent_missing");
    assert_eq!(result["automatic_resubmission_permitted"], false);
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(f.calls("scheduler.jsonl").is_empty());
}
