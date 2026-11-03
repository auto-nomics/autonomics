//! Durable account-job monitoring. Polls only owned, exactly bound existing jobs.
//! Notifications are an outbox: terminal evidence is saved before delivery and
//! delivery is retried until the owning conversation checkpoints its receipt.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::HpcClient;
use crate::jobs::{REMOTE_JOBS_HELPER, digest, local_record_path, read_local_record};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub agent_id: String,
    pub agent_path: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorRecord {
    pub version: u32,
    pub id: String,
    pub owner: Owner,
    pub ssh_host: String,
    pub cluster: String,
    pub submission_key: String,
    pub nonce: String,
    pub request_sha256: String,
    pub job_id: Option<String>,
    pub state: String,
    pub observation: Option<Value>,
    pub notification: String,
    pub verification_plan: Value,
    pub next_poll: u64,
    pub failures: u32,
    pub last_error: Option<String>,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MonitorView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<u64>,
    pub id: String,
    pub agent_id: String,
    pub agent_path: String,
    pub session_id: String,
    pub submission_key: String,
    pub cluster: String,
    pub job_id: Option<String>,
    pub state: String,
    pub scheduler_state: Option<String>,
    pub notification: String,
    pub last_error: Option<String>,
    pub next_poll: u64,
    pub updated_at: u64,
    pub scientific_admission_granted: bool,
}

impl MonitorView {
    /// An unresolved submission is not evidence of a running job. Keep its
    /// recovery/identity checks intact, but surface the need for intervention.
    pub fn needs_attention(&self) -> bool {
        self.completion_pending()
            && (self.state == "submission_uncertain"
                || self.last_error.as_deref().is_some_and(|error| {
                    // Older releases persisted a normal acknowledgement wait
                    // as an error. Keep those outboxes recoverable on upgrade.
                    !(self.state == "terminal"
                        && self.notification == "pending"
                        && error == "notification_pending_original_context")
                }))
    }
    /// Work remains outstanding until the original conversation has accepted
    /// its terminal notice. A lost connection or UNKNOWN submission is not done.
    pub fn completion_pending(&self) -> bool {
        matches!(
            self.state.as_str(),
            "awaiting_submission" | "watching" | "submission_uncertain" | "terminal"
        ) && self.notification != "delivered"
    }
}

impl MonitorRecord {
    fn active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "awaiting_submission" | "watching" | "submission_uncertain" | "terminal"
        ) && self.notification != "delivered"
    }
    pub fn view(&self) -> MonitorView {
        let status = self.observation.as_ref();
        let state = status
            .and_then(|v| v["accounting"].as_array())
            .and_then(|rows| {
                rows.iter()
                    .find(|r| r["job_id"] == self.job_id.as_deref().unwrap_or(""))
            })
            .and_then(|r| r["state"].as_str());
        MonitorView {
            target_id: None,
            target_revision: None,
            id: self.id.clone(),
            agent_id: self.owner.agent_id.clone(),
            agent_path: self.owner.agent_path.clone(),
            session_id: self.owner.session_id.clone(),
            submission_key: self.submission_key.clone(),
            cluster: self.cluster.clone(),
            job_id: self.job_id.clone(),
            state: self.state.clone(),
            scheduler_state: state
                .map(str::to_owned)
                .or_else(|| status.and_then(|v| v["state"].as_str()).map(str::to_owned)),
            notification: self.notification.clone(),
            last_error: self.last_error.clone(),
            next_poll: self.next_poll,
            updated_at: self.updated_at,
            scientific_admission_granted: false,
        }
    }
    fn notice(&self) -> String {
        // No remote log text or script contents become instructions to the agent.
        let evidence = json!({"event_id":format!("hpc-job-{}",self.id),"monitor_id":self.id,"job_id":self.job_id,"cluster":self.cluster,"submission_key":self.submission_key,"status":self.observation.as_ref().map(|v|json!({"job_id":v["job_id"],"state":v["state"],"successful":v["successful"],"terminal":v["terminal"],"ownership_verified":v["ownership_verified"],"stop_confirmed":v["stop_confirmed"],"accounting_steps":v["accounting"].as_array().map(|r|r.len())})),"verification_plan_at_submission":self.verification_plan.to_string().chars().take(8000).collect::<String>(),"scientific_admission_granted":false});
        format!(
            "[HPC job terminal notification]\n{}\nThe existing job has verified owned terminal accounting and has stopped. Continue the original task by inspecting its logs and validating the existing expected outputs against the frozen contract, hashes and QC requirements. Scheduler success alone is not output acceptance. Keep readiness/scientific authorization unchanged until their existing acceptance requirements are satisfied. Do not submit, resubmit, requeue, rerun a computation or start downstream scientific analysis in response to this notification. Report failures and missing outputs truthfully. This event is bound to this original conversation; its stable ID prevents duplicate notification injection.",
            evidence
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionDelivery {
    /// The original context has durably completed its verification turn.
    Delivered,
    /// Queued or processing; retain the outbox and poll the same receipt.
    Pending,
}

#[async_trait]
pub trait CompletionSink: Send + Sync {
    /// Pending is ordinary progress, not a delivery failure. Only Delivered
    /// proves that the exact owning context durably handled the notice.
    async fn deliver(
        &self,
        record: &MonitorRecord,
        event_id: &str,
        message: String,
    ) -> Result<CompletionDelivery, String>;
}

pub struct MonitorHub {
    client: Arc<HpcClient>,
    directory: PathBuf,
    lock: Mutex<()>,
    changed: broadcast::Sender<MonitorView>,
    cadence: Duration,
}

impl MonitorHub {
    pub fn open(client: Arc<HpcClient>, cadence: Duration) -> Result<Arc<Self>, String> {
        let directory = client.state_dir().join("job-monitors");
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let m = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.permissions().mode() & 0o7777 != 0o700
        {
            return Err("HPC monitor directory must be private, owned and nonsymlink".into());
        }
        let (changed, _) = broadcast::channel(256);
        let hub = Arc::new(Self {
            client,
            directory,
            lock: Mutex::new(()),
            changed,
            cadence: cadence.clamp(Duration::from_secs(30), Duration::from_secs(300)),
        });
        hub.records()?; // refuse corrupt/rebound persistent state before service starts
        Ok(hub)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<MonitorView> {
        self.changed.subscribe()
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid HPC monitor identity".into());
        }
        Ok(self.directory.join(format!("{id}.json")))
    }
    fn read(&self, id: &str) -> Result<Option<MonitorRecord>, String> {
        read_local_record(&self.path(id)?)?
            .map(|v| {
                let r: MonitorRecord =
                    serde_json::from_value(v).map_err(|_| "invalid HPC monitor record")?;
                if r.version != 1
                    || r.id != id
                    || r.id != digest(format!("{}\0{}", r.ssh_host, r.nonce).as_bytes())
                    || uuid::Uuid::parse_str(&r.owner.agent_id).is_err()
                    || uuid::Uuid::parse_str(&r.owner.session_id).is_err()
                    || !(r.owner.agent_path == "/root" || r.owner.agent_path.starts_with("/root/"))
                    || !matches!(
                        r.state.as_str(),
                        "awaiting_submission"
                            | "watching"
                            | "submission_uncertain"
                            | "terminal"
                            | "not_submitted"
                            | "followup_only"
                            | "stopped"
                    )
                    || !matches!(r.notification.as_str(), "none" | "pending" | "delivered")
                {
                    return Err("HPC monitor binding is invalid".into());
                }
                Ok(r)
            })
            .transpose()
    }
    fn save(&self, r: &mut MonitorRecord) -> Result<(), String> {
        r.revision = r
            .revision
            .checked_add(1)
            .ok_or("monitor revision overflow")?;
        r.updated_at = now();
        let path = self.path(&r.id)?;
        let temporary = self
            .directory
            .join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let written = (|| {
            file.write_all(&serde_json::to_vec(r).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
            fs::File::open(&self.directory)
                .and_then(|d| d.sync_all())
                .map_err(|e| e.to_string())
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written?;
        let _ = self.changed.send(r.view());
        Ok(())
    }
    pub fn records(&self) -> Result<Vec<MonitorRecord>, String> {
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("invalid monitor file name")?;
            if name.starts_with(".pending-") {
                continue;
            }
            let id = name
                .strip_suffix(".json")
                .ok_or("unexpected file in monitor directory")?;
            records.push(self.read(id)?.ok_or("monitor record disappeared")?);
            if records.len() > 10000 {
                return Err("HPC monitor inventory requires operator maintenance".into());
            }
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(records)
    }
    pub fn views(&self, owner: Option<&str>) -> Result<Vec<MonitorView>, String> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|r| owner.is_none_or(|p| r.owner.agent_path == p))
            .map(|r| r.view())
            .collect())
    }
    fn intent_for(&self, r: &MonitorRecord) -> Result<(HpcClient, Value), String> {
        let client = self.client.for_cluster(Some(&r.cluster))?;
        let value = read_local_record(&local_record_path(&client, &r.submission_key)?)?
            .ok_or("monitored local intent is missing")?;
        if client.config().ssh_host != r.ssh_host
            || value["version"] != 1
            || value["nonce"] != r.nonce
            || value["submission_key"] != r.submission_key
            || value["ssh_host"] != r.ssh_host
            || value["request_sha256"] != r.request_sha256
            || digest(&serde_json::to_vec(&value["request"]).map_err(|e| e.to_string())?)
                != r.request_sha256
        {
            return Err("monitored local intent identity changed".into());
        }
        Ok((client, value))
    }
    pub fn prepare(
        &self,
        client: &HpcClient,
        cluster: Option<&str>,
        intent: &Value,
        owner: Owner,
        plan: Value,
    ) -> Result<MonitorRecord, String> {
        let id = digest(
            format!(
                "{}\0{}",
                client.config().ssh_host,
                intent["nonce"].as_str().ok_or("missing intent nonce")?
            )
            .as_bytes(),
        );
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        if let Some(mut old) = self.read(&id)? {
            if old.owner != owner {
                return Err("submission monitor belongs to another agent or context".into());
            }
            self.intent_for(&old)?;
            if old.state == "not_submitted"
                && old.job_id.is_none()
                && old.notification != "delivered"
            {
                // A new explicit same-key operation must have a durable active
                // observer before SSH, even after a prior PREPARED rejection.
                // Otherwise a lost reply could leave a running job unobserved.
                old.state = "awaiting_submission".into();
                old.next_poll = now() + 120;
                old.last_error = None;
                self.save(&mut old)?;
            }
            return Ok(old);
        }
        if uuid::Uuid::parse_str(&owner.agent_id).is_err()
            || uuid::Uuid::parse_str(&owner.session_id).is_err()
            || !(owner.agent_path == "/root" || owner.agent_path.starts_with("/root/"))
        {
            return Err("invalid monitor owner".into());
        }
        let mut record = MonitorRecord {
            version: 1,
            id,
            owner,
            ssh_host: client.config().ssh_host.clone(),
            cluster: cluster.unwrap_or("default").to_owned(),
            submission_key: intent["submission_key"]
                .as_str()
                .ok_or("missing intent key")?
                .into(),
            nonce: intent["nonce"]
                .as_str()
                .ok_or("missing intent nonce")?
                .into(),
            request_sha256: intent["request_sha256"]
                .as_str()
                .ok_or("missing request identity")?
                .into(),
            job_id: None,
            state: "awaiting_submission".into(),
            observation: None,
            notification: "none".into(),
            verification_plan: plan,
            next_poll: now() + 120,
            failures: 0,
            last_error: None,
            revision: 0,
            created_at: now(),
            updated_at: now(),
        };
        self.intent_for(&record)?;
        self.save(&mut record)?;
        Ok(record)
    }
    pub fn observe_submission(&self, id: &str, result: &Value) -> Result<MonitorRecord, String> {
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        let mut r = self.read(id)?.ok_or("monitor missing after submission")?;
        if r.state == "stopped" || r.notification == "delivered" {
            return Ok(r);
        }
        match result["state"].as_str() {
            Some("SUBMITTED") => {
                let job = result["job_id"]
                    .as_str()
                    .filter(|id| {
                        !id.is_empty()
                            && id.len() <= 12
                            && !id.starts_with('0')
                            && id.bytes().all(|b| b.is_ascii_digit())
                    })
                    .ok_or("invalid monitored job ID")?;
                if r.job_id.as_deref().is_some_and(|old| old != job) {
                    return Err("monitored job ID cannot be rebound".into());
                }
                r.job_id = Some(job.into());
                r.state = "watching".into();
                r.next_poll = now();
                r.last_error = None;
            }
            Some("REJECTED" | "PREPARED") if r.job_id.is_none() => {
                r.state = "not_submitted".into();
                r.observation = Some(result.clone());
                r.last_error = result["reason"].as_str().map(str::to_owned);
            }
            Some("OPERATOR_FOLLOWUP_COMPLETED" | "OPERATOR_FOLLOWUP_UNVERIFIED") => {
                r.state = "followup_only".into();
            }
            _ => {
                r.state = "submission_uncertain".into();
                r.last_error = Some(
                    result["reason"]
                        .as_str()
                        .unwrap_or("submission_not_resolved_no_retry")
                        .into(),
                );
                r.next_poll = now() + self.cadence.as_secs();
            }
        }
        self.save(&mut r)?;
        Ok(r)
    }
    /// Operator/agent attachment to an already durable submission; never submit.
    pub async fn watch_existing(
        &self,
        cluster: Option<&str>,
        key: &str,
        owner: Owner,
        plan: Value,
    ) -> Result<MonitorRecord, String> {
        let client = self.client.for_cluster(cluster)?;
        let intent = crate::jobs::intent(&client, key, None)?;
        let record = self.prepare(&client, cluster, &intent, owner.clone(), plan)?;
        let record = self.resume(&record.id, &owner)?;
        let recovered = self
            .query(
                &client,
                json!({"tool":"jobs","action":"recover","nonce":intent["nonce"],"expected_request":intent["request"]}),
            )
            .await?;
        self.observe_submission(&record.id, &recovered)
    }

    /// Explicitly resume only the same monitor and owner; delivered notices stay deduplicated.
    pub fn resume(&self, id: &str, owner: &Owner) -> Result<MonitorRecord, String> {
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        let mut r = self.read(id)?.ok_or("monitor not found")?;
        if &r.owner != owner {
            return Err("monitor belongs to another context".into());
        }
        self.intent_for(&r)?;
        if r.state == "stopped" && r.notification != "delivered" {
            r.state = if r.job_id.is_some() {
                "watching"
            } else {
                "submission_uncertain"
            }
            .into();
            r.next_poll = now();
            self.save(&mut r)?;
        }
        Ok(r)
    }
    pub fn stop(&self, id: &str, owner: &Owner) -> Result<MonitorRecord, String> {
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        let mut r = self.read(id)?.ok_or("monitor not found")?;
        if &r.owner != owner {
            return Err("monitor belongs to another context".into());
        }
        r.state = "stopped".into();
        self.save(&mut r)?;
        Ok(r)
    }
    fn revise(
        &self,
        prior: &MonitorRecord,
        edit: impl FnOnce(&mut MonitorRecord),
    ) -> Result<Option<MonitorRecord>, String> {
        let _lock = self.lock.lock().map_err(|_| "HPC monitor lock poisoned")?;
        let mut r = self.read(&prior.id)?.ok_or("monitor disappeared")?;
        if r.revision != prior.revision || !r.active() {
            return Ok(None);
        }
        edit(&mut r);
        self.save(&mut r)?;
        Ok(Some(r))
    }
    async fn query(&self, client: &HpcClient, input: Value) -> Result<Value, String> {
        let result = client.run_json(REMOTE_JOBS_HELPER, &input, false).await?;
        if result["ok"] != true {
            return Err("remote job verification unavailable".into());
        }
        Ok(result)
    }
    pub async fn poll(
        &self,
        record: MonitorRecord,
        sink: &dyn CompletionSink,
    ) -> Result<(), String> {
        let checked=async {
            let (client,intent)=self.intent_for(&record)?;
            if record.job_id.is_none(){
                let recovery=self.query(&client,json!({"tool":"jobs","action":"recover","nonce":record.nonce,"expected_request":intent["request"]})).await?;
                // Recover is read-only: it never calls sbatch to retry an uncertain attempt.
                if recovery["state"]=="SUBMITTED" {
                    let _lock=self.lock.lock().map_err(|_|"HPC monitor lock poisoned")?;
                    let mut latest=self.read(&record.id)?.ok_or("monitor disappeared")?;
                    if latest.revision==record.revision && latest.active(){
                        let job=recovery["job_id"].as_str().ok_or("missing recovered job")?;
                        if job.len()>12 || job.is_empty() || job.starts_with('0') || !job.bytes().all(|b|b.is_ascii_digit()){return Err("invalid recovered job".into());}
                        latest.job_id=Some(job.into());latest.state="watching".into();latest.next_poll=now();latest.failures=0;latest.last_error=None;self.save(&mut latest)?;
                    }
                    return Ok(());
                }
                if matches!(recovery["state"].as_str(), Some("REJECTED" | "PREPARED")) {
                    // PREPARED is an exact durable remote proof that sbatch
                    // has not been attempted. Missing intent/UNKNOWN is not.
                    self.revise(&record, |r| {
                        r.state = "not_submitted".into();
                        r.observation = Some(recovery.clone());
                        r.last_error = recovery["reason"].as_str().map(str::to_owned);
                    })?;
                    return Ok(());
                }
                self.revise(&record,|r|{r.state="submission_uncertain".into();r.last_error=Some(recovery["reason"].as_str().unwrap_or("submission_not_resolved_no_retry").into());r.next_poll=now()+300;})?;
                return Ok(());
            }
            let verified=self.query(&client,json!({"tool":"jobs","action":"verify_job_intent","nonce":record.nonce,"expected_request":intent["request"],"expected_job_id":record.job_id})).await?;
            if verified["remote_intent_verified"]!=true || verified["nonce"]!=record.nonce || verified["job_id"]!=record.job_id.as_deref().unwrap_or("") || verified["script_sha256"]!=digest(intent["request"]["script"].as_str().ok_or("missing monitored script")?.as_bytes()){return Err("remote job binding changed".into());}
            let status=&verified["status"];
            let terminal=status["ownership_verified"]==true && status["stop_confirmed"]==true && status["terminal"]==true;
            let Some(mut latest)=self.revise(&record,|r|{r.observation=Some(status.clone());r.state=if terminal{"terminal"}else{"watching"}.into();if r.notification!="pending"{r.failures=0;}r.last_error=None;r.next_poll=now()+self.cadence.as_secs();if terminal{r.notification="pending".into();}})? else{return Ok(());};
            if terminal {
                let event=format!("hpc-job-{}",latest.id);
                // Reverify remote terminal evidence on every outbox retry, including restart.
                match sink.deliver(&latest,&event,latest.notice()).await {
                    Ok(CompletionDelivery::Delivered)=>{self.revise(&latest,|r|{r.notification="delivered".into();r.last_error=None;r.failures=0;})?;}
                    Ok(CompletionDelivery::Pending)=>{self.revise(&latest,|r|{r.last_error=None;r.failures=0;})?;}
                    Err(error)=>{self.revise(&latest,|r|{r.last_error=Some(format!("notification_delivery_failed: {error}"));r.failures=r.failures.saturating_add(1);r.next_poll=now()+self.cadence.as_secs().saturating_mul(1u64<<r.failures.min(4)).min(900);})?;}
                }
                latest.observation=None; // do not retain an extra large status across the next record
            }
            Ok(())
        }.await;
        if checked.is_err() {
            self.revise(&record, |r| {
                r.last_error = Some("remote_or_identity_verification_unavailable".into());
                r.failures = r.failures.saturating_add(1);
                r.next_poll = now()
                    + self
                        .cadence
                        .as_secs()
                        .saturating_mul(1u64 << r.failures.min(4))
                        .min(900);
            })?;
        }
        checked
    }
    pub fn start(self: &Arc<Self>, sink: Arc<dyn CompletionSink>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(hub) = weak.upgrade() else {
                    break;
                };
                if let Ok(records) = hub.records() {
                    let due: Vec<_> = records
                        .into_iter()
                        .filter(|r| r.active() && r.next_poll <= now())
                        .take(32)
                        .collect();
                    futures::stream::iter(due)
                        .for_each_concurrent(4, |record| {
                            let hub = hub.clone();
                            let sink = sink.clone();
                            async move {
                                let _ = hub.poll(record, sink.as_ref()).await;
                            }
                        })
                        .await;
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;
