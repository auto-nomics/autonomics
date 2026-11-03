use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    _temp: tempfile::TempDir,
    client: Arc<HpcClient>,
    hub: Arc<MonitorHub>,
    owner: Owner,
    intent: Value,
    flags: PathBuf,
    calls: PathBuf,
}
fn private(path: &std::path::Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let flags = temp.path().join("flags");
        let calls = temp.path().join("calls");
        let ssh = temp.path().join("ssh");
        private(&flags, &json!({}));
        let code = format!(
            r#"#!/usr/bin/env python3
import json,sys,hashlib
r=json.load(sys.stdin)['input'];flags=json.load(open({flags}))
with open({calls},'a') as f:f.write(json.dumps(r)+'\n')
assert r['action'] in ['recover','verify_job_intent'], 'a monitor must NEVER submit'
if r['action']=='recover':
 print(json.dumps({{'ok':True,'state':flags.get('recovery_state', 'UNKNOWN' if flags.get('unknown') else 'SUBMITTED'),'reason':flags.get('reason'),'nonce':r['nonce'],'job_id':'123','submission_retried':False}}))
else:
 assert r['expected_job_id']=='123' and r['expected_request']['script']=='#!/bin/sh\ntrue\n'
 done=not flags.get('running');failed=flags.get('failed',False)
 state='FAILED' if failed else 'COMPLETED' if done else 'RUNNING'
 print(json.dumps({{'ok':not flags.get('unavailable'),'remote_intent_verified':True,'nonce':'b'*32 if flags.get('rebound') else r['nonce'],'job_id':'123',
 'script_sha256':hashlib.sha256(r['expected_request']['script'].encode()).hexdigest(),
 'status':{{'job_id':'123','state':'TERMINAL' if done else 'ACTIVE','ownership_verified':True,'terminal':done,'stop_confirmed':done and not flags.get('missing_steps'),'successful':done and not failed,
 'accounting':[{{'job_id':'123','job_id_raw':'123','state':state,'exit_code':'1:0' if failed else '0:0'}}]}}}}))
"#,
            flags = serde_json::to_string(&flags).unwrap(),
            calls = serde_json::to_string(&calls).unwrap()
        );
        fs::write(&ssh, code).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let client = Arc::new(
            HpcClient::new(crate::HpcConfig {
                ssh_host: "toy".into(),
                data_ssh_host: None,
                pi_ssh_host: None,
                ssh_command: ssh.to_string_lossy().into_owned(),
                remote_python: "python3".into(),
                state_dir: temp.path().join("state"),
                default_partition: "cpu".into(),
                default_account: None,
            })
            .unwrap(),
        );
        let request = json!({"tool":"jobs","action":"submit","submission_key":"one","script":"#!/bin/sh\ntrue\n","partition":"cpu","working_directory":"/tmp/toy"});
        let intent = json!({"version":1,"ssh_host":"toy","submission_key":"one","nonce":"a".repeat(32),"request_sha256":digest(&serde_json::to_vec(&request).unwrap()),"request":request});
        private(&local_record_path(&client, "one").unwrap(), &intent);
        let hub = MonitorHub::open(client.clone(), Duration::from_secs(60)).unwrap();
        let owner = Owner {
            agent_id: uuid::Uuid::new_v4().to_string(),
            agent_path: "/root/toy".into(),
            session_id: uuid::Uuid::new_v4().to_string(),
        };
        Self {
            _temp: temp,
            client,
            hub,
            owner,
            intent,
            flags,
            calls,
        }
    }
    fn prepared(&self) -> MonitorRecord {
        self.hub
            .prepare(
                &self.client,
                None,
                &self.intent,
                self.owner.clone(),
                json!({"step":"validate seven frozen outputs"}),
            )
            .unwrap()
    }
    fn submitted(&self) -> MonitorRecord {
        let r = self.prepared();
        self.hub
            .observe_submission(&r.id, &json!({"state":"SUBMITTED","job_id":"123"}))
            .unwrap()
    }
    fn flags(&self, v: Value) {
        private(&self.flags, &v);
    }
    fn record(&self) -> MonitorRecord {
        self.hub.records().unwrap().remove(0)
    }
    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.calls)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
#[derive(Default)]
struct Sink {
    fail: AtomicBool,
    pending: AtomicBool,
    events: Mutex<Vec<String>>,
    messages: Mutex<Vec<String>>,
}
#[async_trait]
impl CompletionSink for Sink {
    async fn deliver(
        &self,
        _: &MonitorRecord,
        id: &str,
        message: String,
    ) -> Result<CompletionDelivery, String> {
        self.events.lock().unwrap().push(id.to_owned());
        self.messages.lock().unwrap().push(message);
        if self.fail.load(Ordering::Relaxed) {
            Err("lost acknowledgement".into())
        } else if self.pending.load(Ordering::Relaxed) {
            Ok(CompletionDelivery::Pending)
        } else {
            Ok(CompletionDelivery::Delivered)
        }
    }
}

#[tokio::test]
async fn prepare_is_durable_before_transport_and_lost_submit_reply_is_only_recovered() {
    let f = Fixture::new();
    let r = f.prepared();
    assert!(!f.calls.exists());
    assert_eq!(f.record().state, "awaiting_submission");
    let reopened = MonitorHub::open(f.client.clone(), Duration::from_secs(60)).unwrap();
    reopened.poll(r, &Sink::default()).await.unwrap();
    let r = f.record();
    assert_eq!(r.job_id.as_deref(), Some("123"));
    assert_eq!(r.state, "watching");
    let calls = f.requests();
    assert!(calls.iter().any(|r| r["action"] == "recover"));
    assert!(calls.iter().all(|r| r["action"] != "submit"));
    assert!(
        calls
            .iter()
            .all(|r| r["expected_request"] == f.intent["request"])
    );
}
#[tokio::test]
async fn terminal_notification_is_durable_and_delivered_after_restart() {
    let f = Fixture::new();
    let r = f.submitted();
    let sink = Sink::default();
    sink.fail.store(true, Ordering::Relaxed);
    f.hub.poll(r, &sink).await.unwrap();
    let pending = f.record();
    assert_eq!(pending.state, "terminal");
    assert_eq!(pending.notification, "pending");
    assert!(pending.view().needs_attention());
    assert!(pending.observation.unwrap()["stop_confirmed"] == true);
    let reopened = MonitorHub::open(f.client.clone(), Duration::from_secs(60)).unwrap();
    sink.fail.store(false, Ordering::Relaxed);
    reopened.poll(f.record(), &sink).await.unwrap();
    assert_eq!(f.record().notification, "delivered");
    assert_eq!(f.record().last_error, None);
    assert_eq!(f.record().failures, 0);
    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], events[1]);
    let notice = sink.messages.lock().unwrap();
    assert!(notice[0].contains("Do not submit, resubmit, requeue, rerun"));
    assert!(notice[0].contains("scientific_admission_granted\":false"));
}
#[tokio::test]
async fn queued_verification_is_waiting_and_delivery_broadcast_clears_it() {
    let f = Fixture::new();
    let sink = Sink::default();
    sink.pending.store(true, Ordering::Relaxed);
    f.hub.poll(f.submitted(), &sink).await.unwrap();
    for _ in 0..3 {
        let pending = f.record();
        assert_eq!(pending.state, "terminal");
        assert_eq!(pending.notification, "pending");
        assert_eq!(pending.last_error, None);
        assert_eq!(pending.failures, 0);
        assert!(pending.view().completion_pending());
        assert!(!pending.view().needs_attention());
        assert!(pending.next_poll <= now() + 60);
        f.hub.poll(pending, &sink).await.unwrap();
    }
    let mut updates = f.hub.subscribe();
    sink.pending.store(false, Ordering::Relaxed);
    f.hub.poll(f.record(), &sink).await.unwrap();
    let mut last = None;
    while let Ok(view) = updates.try_recv() {
        last = Some(view);
    }
    let delivered = last.unwrap();
    assert_eq!(delivered.notification, "delivered");
    assert!(!delivered.completion_pending());
    assert!(!delivered.needs_attention());
    assert_eq!(delivered.last_error, None);
    let reopened = MonitorHub::open(f.client.clone(), Duration::from_secs(60)).unwrap();
    assert!(!reopened.records().unwrap()[0].view().completion_pending());
    assert!(
        f.requests()
            .iter()
            .all(|r| r["action"] == "verify_job_intent")
    );
}

#[tokio::test]
async fn legacy_pending_error_recovers_without_hiding_verification_failures() {
    let f = Fixture::new();
    let mut record = f.submitted();
    record.state = "terminal".into();
    record.notification = "pending".into();
    record.last_error = Some("notification_pending_original_context".into());
    record.failures = 2;
    f.hub.save(&mut record).unwrap();
    assert!(record.view().completion_pending());
    assert!(!record.view().needs_attention());
    let sink = Sink::default();
    sink.pending.store(true, Ordering::Relaxed);
    f.hub.poll(record, &sink).await.unwrap();
    assert_eq!(f.record().last_error, None);
    assert_eq!(f.record().failures, 0);
    f.flags(json!({"rebound":true}));
    assert!(f.hub.poll(f.record(), &sink).await.is_err());
    assert!(f.record().view().needs_attention());
    assert_ne!(f.record().notification, "delivered");
}

#[tokio::test]
async fn running_jobs_leave_the_conversation_idle_without_a_completion_event() {
    let f = Fixture::new();
    f.flags(json!({"running":true}));
    let sink = Sink::default();
    f.hub.poll(f.submitted(), &sink).await.unwrap();
    assert_eq!(f.record().state, "watching");
    assert_eq!(f.record().notification, "none");
    assert!(sink.events.lock().unwrap().is_empty());
}
#[tokio::test]
async fn failed_jobs_notify_truthful_failure_and_grant_no_scientific_admission() {
    let f = Fixture::new();
    f.flags(json!({"failed":true}));
    let sink = Sink::default();
    f.hub.poll(f.submitted(), &sink).await.unwrap();
    let r = f.record();
    assert_eq!(r.notification, "delivered");
    assert_eq!(r.observation.as_ref().unwrap()["successful"], false);
    assert!(!r.view().scientific_admission_granted);
}
#[tokio::test]
async fn incomplete_stop_proof_or_transport_failure_never_becomes_completion() {
    for flags in [
        json!({"missing_steps":true}),
        json!({"unavailable":true}),
        json!({"rebound":true}),
    ] {
        let f = Fixture::new();
        f.flags(flags);
        let sink = Sink::default();
        let _ = f.hub.poll(f.submitted(), &sink).await;
        assert_ne!(f.record().notification, "delivered");
        assert!(sink.events.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn identity_changes_and_cross_context_adoption_fail_closed() {
    let f = Fixture::new();
    let r = f.submitted();
    let mut foreign = f.owner.clone();
    foreign.session_id = uuid::Uuid::new_v4().to_string();
    assert!(
        f.hub
            .prepare(&f.client, None, &f.intent, foreign.clone(), Value::Null)
            .is_err()
    );
    assert!(f.hub.stop(&r.id, &foreign).is_err());
    let mut changed = f.intent.clone();
    changed["request"]["script"] = json!("#!/bin/sh\nfalse\n");
    private(&local_record_path(&f.client, "one").unwrap(), &changed);
    assert!(f.hub.poll(r, &Sink::default()).await.is_err());
    assert_eq!(
        f.record().last_error.as_deref(),
        Some("remote_or_identity_verification_unavailable")
    );
}
#[tokio::test]
async fn stopping_a_monitor_does_not_cancel_the_job_or_deliver_a_notice() {
    let f = Fixture::new();
    let r = f.submitted();
    let stopped = f.hub.stop(&r.id, &f.owner).unwrap();
    assert_eq!(stopped.state, "stopped");
    assert!(!stopped.active());
    let sink = Sink::default();
    f.hub.poll(r, &sink).await.unwrap();
    assert!(sink.events.lock().unwrap().is_empty());
    assert_eq!(f.record().state, "stopped");
}
#[tokio::test]
async fn old_unknown_remains_unresolved_without_resubmission() {
    let f = Fixture::new();
    f.flags(json!({"unknown":true}));
    let sink = Sink::default();
    f.hub.poll(f.prepared(), &sink).await.unwrap();
    assert_eq!(f.record().state, "submission_uncertain");
    assert!(f.record().job_id.is_none());
    assert!(f.record().view().needs_attention());
    assert!(sink.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovered_job_clears_submission_error_and_returns_to_normal_waiting() {
    let f = Fixture::new();
    f.flags(json!({"unknown":true}));
    let sink = Sink::default();
    f.hub.poll(f.prepared(), &sink).await.unwrap();
    assert!(f.record().view().needs_attention());
    f.flags(json!({"running":true}));
    f.hub.poll(f.record(), &sink).await.unwrap();
    assert_eq!(f.record().job_id.as_deref(), Some("123"));
    assert_eq!(f.record().state, "watching");
    assert!(!f.record().view().needs_attention());
    assert!(f.record().view().completion_pending());
    assert!(sink.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn exact_prepared_recovery_converges_without_claiming_job_completion() {
    let f = Fixture::new();
    f.flags(json!({"recovery_state":"PREPARED", "reason":"submission_not_attempted"}));
    let sink = Sink::default();
    f.hub.poll(f.prepared(), &sink).await.unwrap();
    let r = f.record();
    assert_eq!(r.state, "not_submitted");
    assert_eq!(r.last_error.as_deref(), Some("submission_not_attempted"));
    assert!(!r.active());
    assert!(!r.view().completion_pending());
    assert!(r.job_id.is_none());
    assert_ne!(r.notification, "delivered");
    assert!(sink.events.lock().unwrap().is_empty());
    let reopened = MonitorHub::open(f.client.clone(), Duration::from_secs(60)).unwrap();
    assert_eq!(reopened.records().unwrap()[0].state, "not_submitted");
}

#[tokio::test]
async fn explicit_same_key_prepare_rearms_a_rejected_monitor_before_transport() {
    let f = Fixture::new();
    let original = f.prepared();
    f.hub
        .observe_submission(
            &original.id,
            &json!({"state":"REJECTED", "reason":"submission_validation_failed"}),
        )
        .unwrap();
    assert!(!f.record().active());
    let rearmed = f.prepared();
    assert_eq!(rearmed.state, "awaiting_submission");
    assert!(rearmed.active());
    assert_eq!(rearmed.nonce, original.nonce);
    assert_eq!(rearmed.request_sha256, original.request_sha256);
    assert!(!f.calls.exists());
    // Simulate loss of the subsequent explicit submit reply and a restart:
    // only exact recovery is performed; the monitor cannot retry a submit.
    let reopened = MonitorHub::open(f.client.clone(), Duration::from_secs(60)).unwrap();
    reopened.poll(f.record(), &Sink::default()).await.unwrap();
    assert_eq!(f.record().state, "watching");
    assert_eq!(f.record().job_id.as_deref(), Some("123"));
    assert!(f.requests().iter().all(|r| r["action"] == "recover"));
}

#[tokio::test]
async fn missing_intent_and_unavailable_recovery_remain_uncertain() {
    for (state, reason) in [
        ("UNKNOWN", "remote_intent_missing"),
        ("UNAVAILABLE", "scheduler_unavailable"),
    ] {
        let f = Fixture::new();
        f.flags(json!({"recovery_state":state, "reason":reason}));
        let sink = Sink::default();
        f.hub.poll(f.prepared(), &sink).await.unwrap();
        let r = f.record();
        assert_eq!(r.state, "submission_uncertain");
        assert_eq!(r.last_error.as_deref(), Some(reason));
        assert!(r.view().needs_attention());
        assert!(r.active());
        assert!(sink.events.lock().unwrap().is_empty());
        assert!(f.requests().iter().all(|r| r["action"] != "submit"));
    }
}
#[test]
fn unsafe_or_corrupt_persistent_records_are_not_adopted() {
    let f = Fixture::new();
    let r = f.prepared();
    fs::set_permissions(
        f.hub.path(&r.id).unwrap(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(f.hub.records().is_err());
}

#[tokio::test]
async fn explicit_watch_resumes_same_stopped_monitor_without_submitting_or_changing_owner() {
    let f = Fixture::new();
    let r = f.submitted();
    f.hub.stop(&r.id, &f.owner).unwrap();
    assert_eq!(f.record().state, "stopped");
    let r = f
        .hub
        .watch_existing(None, "one", f.owner.clone(), json!({}))
        .await
        .unwrap();
    assert_eq!(r.state, "watching");
    assert_eq!(r.job_id.as_deref(), Some("123"));
    let mut foreign = f.owner.clone();
    foreign.session_id = uuid::Uuid::new_v4().to_string();
    assert!(f.hub.resume(&r.id, &foreign).is_err());
    assert!(f.requests().iter().all(|r| r["action"] != "submit"));
}

#[test]
fn root_agent_can_own_a_durable_submission() {
    let f = Fixture::new();
    let mut owner = f.owner.clone();
    owner.agent_path = "/root".into();
    let r = f
        .hub
        .prepare(&f.client, None, &f.intent, owner.clone(), json!({}))
        .unwrap();
    assert_eq!(r.owner, owner);
    assert_eq!(f.record().owner, owner);
}
