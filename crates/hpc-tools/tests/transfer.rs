//! Transfer integration tests use a local Python SSH double and self-cleaning
//! toy VFS/remote directories. No real account, network, or study data is read.
use agentik_core::tools::ToolFunction;
use agentik_sdk::types::ToolResult;
use base64::{Engine, engine::general_purpose::STANDARD};
use hpc_tools::transfer::HpcTransferTool;
use hpc_tools::{HpcClient, HpcConfig, files};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use vfs::OpendalFileStorage;
use vfs::permission::VfsPrincipal;

const CHUNK: usize = 1024 * 1024;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn binary(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| ((index * 17 + index / 251) % 256) as u8)
        .collect()
}

struct Fixture {
    root: tempfile::TempDir,
    remote: PathBuf,
    storage: Arc<OpendalFileStorage>,
    client: Arc<HpcClient>,
    tool: HpcTransferTool,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().expect("toy fixture");
        let remote = root.path().join("remote");
        fs::create_dir(&remote).expect("toy remote directory");
        fs::write(root.path().join("mode"), mode).expect("toy mode");
        let executable = root.path().join("fake-ssh");
        fs::write(&executable, r#"#!/usr/bin/env python3
import base64,json,os,sys
root=os.path.dirname(os.path.abspath(__file__))
envelope=json.load(sys.stdin)
request=envelope['input']
assert sys.argv[-2]=='data-toy'
assert 'StrictHostKeyChecking=yes' in sys.argv
assert 'BatchMode=yes' in sys.argv
assert not any(request.get('path','') in arg for arg in sys.argv[1:] if request.get('path'))
chunk=base64.b64decode(request.get('content','')) if request.get('encoding')=='base64' else b''
assert len(chunk)<=1024*1024
with open(os.path.join(root,'observations.jsonl'),'a') as out:
    out.write(json.dumps({'action':request.get('action'),'bytes':len(chunk),'offset':request.get('offset')})+'\n')
captured=[]
exec(envelope['code'], {'REQUEST':request,'__name__':'__hpc_remote__','print':lambda value:captured.append(value)})
assert len(captured)==1
response=json.loads(captured[0])
mode=open(os.path.join(root,'mode')).read()
if mode=='mutate_after_read' and request.get('action')=='read' and request.get('offset')==0 and request.get('limit',0)>0 and not os.path.exists(os.path.join(root,'mutated')):
    with open(request['path'],'r+b') as stream:
        stream.seek(0)
        stream.write(b'!')
        stream.flush()
        os.fsync(stream.fileno())
    open(os.path.join(root,'mutated'),'w').close()
if mode=='current_bytes_only' and request.get('action')=='upload_commit' and response.get('ok'):
    response.update({'committed':False,'status':'CURRENT_TARGET_BYTES_VERIFIED','already_present':True,'current_target_bytes_verified':True,'historical_commit_verified':False,'receipt_authenticity_verified':False})
print(json.dumps(response))
"#).expect("fake SSH program");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).expect("executable");
        let client = Arc::new(
            HpcClient::new(HpcConfig {
                ssh_host: "login-toy".into(),
                data_ssh_host: Some("data-toy".into()),
                pi_ssh_host: None,
                ssh_command: executable.to_string_lossy().into_owned(),
                remote_python: "python3".into(),
                state_dir: root.path().join("state"),
                default_partition: "cpu".into(),
                default_account: None,
            })
            .expect("toy client"),
        );
        // Use a credential-carrying view, not the VFS superuser identity.
        let storage = Arc::new(
            OpendalFileStorage::new_temp().with_principal(VfsPrincipal::new(10_001, 10_001)),
        );
        let tool = HpcTransferTool {
            client: client.clone(),
            storage: storage.clone(),
        };
        Self {
            root,
            remote,
            storage,
            client,
            tool,
        }
    }

    fn path(&self, name: &str) -> String {
        self.remote.join(name).to_string_lossy().into_owned()
    }

    async fn transfer(
        &self,
        direction: &str,
        remote: &str,
        local: &str,
        extra: Value,
    ) -> ToolResult {
        let mut input =
            json!({"direction":direction,"remote_path":self.path(remote),"vfs_path":local});
        input
            .as_object_mut()
            .expect("input")
            .extend(extra.as_object().expect("extra").clone());
        self.tool.execute(input).await.expect("tool dispatch")
    }

    async fn remote_request(&self, mut input: Value, name: &str) -> Value {
        input["path"] = json!(self.path(name));
        let result = files::request(&self.client, &input)
            .await
            .expect("remote helper transport");
        assert_eq!(result["ok"], true, "{result}");
        result
    }

    async fn vfs_bytes(&self, name: &str, length: usize) -> Vec<u8> {
        self.storage
            .read_range(name, 0..length as u64)
            .await
            .expect("VFS read")
            .to_vec()
    }

    async fn assert_vfs_empty(&self) {
        let entries = self
            .storage
            .resolve("/")
            .list("")
            .await
            .expect("VFS listing")
            .into_iter()
            .filter(|entry| !(entry.path() == "/" && entry.metadata().is_dir()))
            .collect::<Vec<_>>();
        assert!(
            entries.is_empty(),
            "aborted transfer left VFS staging keys: {entries:?}"
        );
    }

    fn observations(&self) -> Vec<Value> {
        fs::read_to_string(self.root.path().join("observations.jsonl"))
            .expect("toy observation log")
            .lines()
            .map(|line| serde_json::from_str(line).expect("observation JSON"))
            .collect()
    }
}

fn success(result: &ToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{}", result.text_content());
    serde_json::from_str(&result.text_content()).expect("structured tool result")
}

#[tokio::test]
async fn binary_multichunk_roundtrip_reports_hashes_without_file_bodies() {
    let fixture = Fixture::new("");
    let bytes = binary(2 * CHUNK + 193);
    fixture
        .storage
        .write_bytes("source.bin", bytes.clone())
        .await
        .expect("VFS source");
    let upload = fixture
        .transfer(
            "upload",
            "remote.bin",
            "source.bin",
            json!({"expected_sha256":sha(&bytes)}),
        )
        .await;
    let uploaded = success(&upload);
    assert_eq!(uploaded["sha256"], sha(&bytes));
    assert_eq!(uploaded["bytes"], bytes.len());
    assert_eq!(
        fs::read(fixture.remote.join("remote.bin")).expect("remote bytes"),
        bytes
    );
    let download = fixture
        .transfer(
            "download",
            "remote.bin",
            "download.bin",
            json!({"expected_sha256":sha(&bytes)}),
        )
        .await;
    let downloaded = success(&download);
    assert_eq!(downloaded["sha256"], sha(&bytes));
    assert_eq!(fixture.vfs_bytes("download.bin", bytes.len()).await, bytes);
    for result in [&upload, &download] {
        assert!(
            result.text_content().len() < 4096,
            "file body leaked into tool transcript"
        );
        assert!(!result.text_content().contains("\"content\""));
    }
    let observations = fixture.observations();
    assert!(
        observations
            .iter()
            .filter(|row| row["action"] == "upload_chunk")
            .count()
            >= 3
    );
    assert!(
        observations
            .iter()
            .filter(|row| row["action"] == "read"
                && row["offset"].as_u64().unwrap_or(0) < bytes.len() as u64)
            .count()
            >= 3
    );
    assert!(
        observations
            .iter()
            .all(|row| row["bytes"].as_u64().unwrap_or(0) <= CHUNK as u64)
    );
}

#[tokio::test]
async fn existing_remote_and_vfs_destinations_are_preserved_without_replacement() {
    let fixture = Fixture::new("");
    fixture
        .storage
        .write_bytes("source", b"new bytes".to_vec())
        .await
        .expect("source");
    fs::write(fixture.remote.join("existing"), b"remote old").expect("remote existing");
    let upload = fixture
        .transfer("upload", "existing", "source", json!({}))
        .await;
    assert_eq!(upload.is_error, Some(true));
    assert_eq!(
        fs::read(fixture.remote.join("existing")).expect("preserved"),
        b"remote old"
    );
    fixture
        .storage
        .write_bytes("existing", b"VFS old".to_vec())
        .await
        .expect("existing VFS");
    let download = fixture
        .transfer("download", "existing", "existing", json!({}))
        .await;
    assert_eq!(download.is_error, Some(true));
    assert_eq!(fixture.vfs_bytes("existing", 7).await, b"VFS old");
}

#[tokio::test]
async fn wrong_expected_content_hash_never_publishes_upload_or_download() {
    let fixture = Fixture::new("");
    let bytes = binary(CHUNK + 73);
    fixture
        .storage
        .write_bytes("source", bytes.clone())
        .await
        .expect("source");
    let upload = fixture
        .transfer(
            "upload",
            "unpublished",
            "source",
            json!({"expected_sha256":"0".repeat(64)}),
        )
        .await;
    assert_eq!(upload.is_error, Some(true));
    assert!(!fixture.remote.join("unpublished").exists());
    fs::write(fixture.remote.join("download-source"), &bytes).expect("remote source");
    let download = fixture
        .transfer(
            "download",
            "download-source",
            "unpublished",
            json!({"expected_sha256":"0".repeat(64)}),
        )
        .await;
    assert_eq!(download.is_error, Some(true));
    assert!(fixture.storage.content_length("unpublished").await.is_err());
    // Only the preexisting upload source may remain, not an incomplete output.
    let entries = fixture
        .storage
        .resolve("/")
        .list("")
        .await
        .expect("VFS listing")
        .into_iter()
        .filter(|entry| !(entry.path() == "/" && entry.metadata().is_dir()))
        .collect::<Vec<_>>();
    assert_eq!(
        entries.len(),
        1,
        "unexpected transfer staging residue: {entries:?}"
    );
    assert_eq!(entries[0].path(), "source");
}

#[tokio::test]
async fn mutation_between_remote_ranges_aborts_vfs_staging() {
    let fixture = Fixture::new("mutate_after_read");
    fs::write(fixture.remote.join("changing"), binary(CHUNK + 511))
        .expect("changing remote source");
    let download = fixture
        .transfer("download", "changing", "unpublished", json!({}))
        .await;
    assert_eq!(download.is_error, Some(true));
    assert!(
        download.text_content().contains("changed") || download.text_content().contains("identity")
    );
    fixture.assert_vfs_empty().await;
}

#[tokio::test]
async fn resume_uses_validated_offset_and_verifies_complete_content() {
    let fixture = Fixture::new("");
    let bytes = binary(CHUNK + 127);
    fixture
        .storage
        .write_bytes("source", bytes.clone())
        .await
        .expect("source");
    let begin = fixture
        .remote_request(json!({"action":"upload_begin"}), "resumed")
        .await;
    let id = begin["upload_id"].as_str().expect("upload ID");
    let prefix = &bytes[..12345];
    fixture.remote_request(json!({"action":"upload_chunk","upload_id":id,"offset":0,"encoding":"base64","content":STANDARD.encode(prefix),"sha256":sha(prefix)}), "resumed").await;
    let uploaded = success(
        &fixture
            .transfer(
                "upload",
                "resumed",
                "source",
                json!({"upload_id":id,"expected_sha256":sha(&bytes)}),
            )
            .await,
    );
    assert_eq!(uploaded["sha256"], sha(&bytes));
    assert_eq!(
        fs::read(fixture.remote.join("resumed")).expect("resumed bytes"),
        bytes
    );
    let observations = fixture.observations();
    assert!(
        observations
            .iter()
            .any(|row| row["action"] == "upload_status")
    );
    assert!(
        observations
            .iter()
            .any(|row| row["action"] == "upload_chunk" && row["offset"] == 12345)
    );
}

#[tokio::test]
async fn changed_vfs_prefix_cannot_publish_a_resumed_remote_upload() {
    let fixture = Fixture::new("");
    let bytes = binary(CHUNK + 127);
    fixture
        .storage
        .write_bytes("source", bytes.clone())
        .await
        .expect("source");
    let begin = fixture
        .remote_request(json!({"action":"upload_begin"}), "mismatch")
        .await;
    let id = begin["upload_id"].as_str().expect("upload ID");
    let wrong_prefix = vec![b'X'; 12345];
    fixture.remote_request(json!({"action":"upload_chunk","upload_id":id,"offset":0,"encoding":"base64","content":STANDARD.encode(&wrong_prefix),"sha256":sha(&wrong_prefix)}), "mismatch").await;
    let uploaded = fixture
        .transfer("upload", "mismatch", "source", json!({"upload_id":id}))
        .await;
    assert_eq!(uploaded.is_error, Some(true));
    assert!(!fixture.remote.join("mismatch").exists());
    assert!(uploaded.text_content().contains("digest") || uploaded.text_content().contains("SHA"));
}

#[tokio::test]
async fn observed_existing_bytes_do_not_become_an_invented_historical_commit() {
    let fixture = Fixture::new("current_bytes_only");
    fixture
        .storage
        .write_bytes("source", b"desired bytes".to_vec())
        .await
        .expect("source");
    let result = success(
        &fixture
            .transfer("upload", "observed", "source", json!({}))
            .await,
    );
    assert_eq!(result["committed"], false);
    assert_eq!(result["current_target_bytes_verified"], true);
    assert_eq!(result["historical_commit_verified"], false);
    assert_eq!(result["receipt_authenticity_verified"], false);
}
