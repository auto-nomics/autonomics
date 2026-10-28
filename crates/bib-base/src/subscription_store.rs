//! P5 durable rules and fenced execution ownership. Network calls never run
//! while holding this connection or a database transaction.
use crate::subscriptions::*;
use bib_types::discovery::SourcePlan;
use std::{sync::Arc, time::Instant};
use tokio::sync::Mutex;
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};
mod pages;
mod runs;
mod scheduling;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bibliographySubscriptionMeta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS bibliographySubscriptions(
 id TEXT PRIMARY KEY,version INTEGER NOT NULL,spec TEXT NOT NULL,
 nextDue INTEGER NOT NULL,lastSuccess INTEGER,owner TEXT,epoch INTEGER NOT NULL,
 leaseUntil INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS bibliographySubscriptions_due ON bibliographySubscriptions(nextDue);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionSources(
 ruleID TEXT NOT NULL REFERENCES bibliographySubscriptions(id) ON DELETE CASCADE,
 source TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(ruleID,source)
);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionSeen(
 ruleID TEXT NOT NULL REFERENCES bibliographySubscriptions(id) ON DELETE CASCADE,
 source TEXT NOT NULL,recordID TEXT NOT NULL,groupID TEXT NOT NULL,
 identity TEXT NOT NULL,bytes INTEGER NOT NULL,
 PRIMARY KEY(ruleID,source,recordID)
);
CREATE INDEX IF NOT EXISTS bibliographySubscriptionSeen_group ON bibliographySubscriptionSeen(ruleID,groupID);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionKeys(
 ruleID TEXT NOT NULL,source TEXT NOT NULL,recordID TEXT NOT NULL,key TEXT NOT NULL,
 PRIMARY KEY(ruleID,key,source,recordID),
 FOREIGN KEY(ruleID,source,recordID) REFERENCES bibliographySubscriptionSeen(ruleID,source,recordID) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS bibliographySubscriptionKeys_record ON bibliographySubscriptionKeys(ruleID,source,recordID);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionCursors(
 ruleID TEXT NOT NULL,source TEXT NOT NULL,cursorHash TEXT NOT NULL,
 PRIMARY KEY(ruleID,source,cursorHash),
 FOREIGN KEY(ruleID,source) REFERENCES bibliographySubscriptionSources(ruleID,source) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionInbox(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,
 ruleID TEXT NOT NULL REFERENCES bibliographySubscriptions(id) ON DELETE CASCADE,
 groupID TEXT NOT NULL,value TEXT NOT NULL,bytes INTEGER NOT NULL,
 UNIQUE(ruleID,groupID)
);
CREATE TABLE IF NOT EXISTS bibliographySubscriptionResourceWait(
 ruleID TEXT PRIMARY KEY REFERENCES bibliographySubscriptions(id) ON DELETE CASCADE,
 since INTEGER NOT NULL,reason TEXT NOT NULL,resourceEpoch INTEGER NOT NULL
);
INSERT OR IGNORE INTO bibliographySubscriptionMeta VALUES('resourceEpoch','0');
CREATE TABLE IF NOT EXISTS bibliographySubscriptionRuns(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,
 ruleID TEXT NOT NULL REFERENCES bibliographySubscriptions(id) ON DELETE CASCADE,
 ruleVersion INTEGER NOT NULL,leaseEpoch INTEGER NOT NULL,started INTEGER NOT NULL,
 finished INTEGER,state TEXT NOT NULL,pagesStarted INTEGER NOT NULL,pagesCommitted INTEGER NOT NULL,
 records INTEGER NOT NULL,notifications INTEGER NOT NULL,value TEXT NOT NULL,bytes INTEGER NOT NULL,
 UNIQUE(ruleID,leaseEpoch)
);
CREATE INDEX IF NOT EXISTS bibliographySubscriptionRuns_rule ON bibliographySubscriptionRuns(ruleID,seq);
CREATE INDEX IF NOT EXISTS bibliographySubscriptionRuns_usage ON bibliographySubscriptionRuns(bytes);
"#;
#[derive(Clone)]
pub struct SubscriptionStore {
    conn: Arc<Mutex<Connection>>,
}
async fn finish<T>(tx: Transaction<'_>, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
fn time(now: i64) -> Result<()> {
    if now < 0 {
        Err(invalid("negative execution time"))
    } else {
        Ok(())
    }
}
fn expiry(now: i64, ttl_ms: u32) -> Result<i64> {
    time(now)?;
    if !(1000..=60_000).contains(&ttl_ms) {
        return Err(invalid("lease TTL must be 1000..60000 ms"));
    }
    now.checked_add(i64::from(ttl_ms))
        .ok_or_else(|| invalid("lease time overflow"))
}
async fn limits(conn: &Connection) -> Result<SubscriptionLimits> {
    let mut rows = conn
        .query(
            "SELECT value FROM bibliographySubscriptionMeta WHERE key='limits'",
            (),
        )
        .await?;
    let value: String = rows
        .next()
        .await?
        .ok_or_else(|| invalid("missing subscription limits"))?
        .get(0)?;
    let result: SubscriptionLimits = serde_json::from_str(&value)?;
    result.validate()?;
    Ok(result)
}
async fn source(conn: &Connection, id: &str, name: &str) -> Result<SubscriptionSourceState> {
    let mut rows = conn
        .query(
            "SELECT value FROM bibliographySubscriptionSources WHERE ruleID=?1 AND source=?2",
            turso::params![id.to_owned(), name.to_owned()],
        )
        .await?;
    let value: String = rows
        .next()
        .await?
        .ok_or(SubscriptionError::NotFound)?
        .get(0)?;
    Ok(serde_json::from_str(&value)?)
}
async fn rule(conn: &Connection, id: &str) -> Result<SubscriptionRule> {
    let mut rows = conn
        .query(
            "SELECT version,spec,nextDue,lastSuccess FROM bibliographySubscriptions WHERE id=?1",
            [id],
        )
        .await?;
    let row = rows.next().await?.ok_or(SubscriptionError::NotFound)?;
    let version: i64 = row.get(0)?;
    let spec: String = row.get(1)?;
    let next_due_ms = row.get(2)?;
    let last_success_ms = row.get(3)?;
    drop(rows);
    let spec: SubscriptionSpec = serde_json::from_str(&spec)?;
    let mut sources = Vec::new();
    for name in &spec.request.sources {
        sources.push(source(conn, id, name).await?);
    }
    Ok(SubscriptionRule {
        id: id.into(),
        version: u64::try_from(version).map_err(|_| invalid("invalid rule version"))?,
        spec,
        next_due_ms,
        last_success_ms,
        sources,
        resource_wait: scheduling::resource_wait(conn, id).await?,
    })
}
async fn fence(conn: &Connection, lease: &SubscriptionLease, now: i64) -> Result<i64> {
    time(now)?;
    let mut rows=conn.query("SELECT leaseUntil FROM bibliographySubscriptions WHERE id=?1 AND version=?2 AND owner=?3 AND epoch=?4 AND leaseUntil>?5",turso::params![lease.rule_id.clone(),lease.version as i64,lease.owner.clone(),lease.epoch as i64,now]).await?;
    Ok(rows
        .next()
        .await?
        .ok_or_else(|| conflict("execution lease expired, changed or was taken over"))?
        .get(0)?)
}
struct OperationClock {
    base: i64,
    started: Instant,
}
impl OperationClock {
    fn new(base: i64) -> Result<Self> {
        time(base)?;
        Ok(Self {
            base,
            started: Instant::now(),
        })
    }
    fn now(&self) -> Result<i64> {
        let elapsed = i64::try_from(self.started.elapsed().as_millis())
            .map_err(|_| invalid("execution clock overflow"))?;
        self.base
            .checked_add(elapsed)
            .ok_or_else(|| invalid("execution clock overflow"))
    }
}
async fn save_source(conn: &Connection, id: &str, state: &SubscriptionSourceState) -> Result<()> {
    let changed = conn
        .execute(
            "UPDATE bibliographySubscriptionSources SET value=?1 WHERE ruleID=?2 AND source=?3",
            turso::params![
                serde_json::to_string(state)?,
                id.to_owned(),
                state.source.clone()
            ],
        )
        .await?;
    if changed != 1 {
        return Err(conflict("source configuration changed"));
    }
    Ok(())
}

impl SubscriptionStore {
    /// Persist a failed attempt without advancing coverage. An expired cursor
    /// can restart only the already frozen plan/window; seen identities survive.
    pub async fn record_failure(
        &self,
        lease: &SubscriptionLease,
        name: &str,
        sequence: u64,
        failure: bib_types::discovery::SourceFailure,
        retry_at_ms: Option<i64>,
        replay_cursor: bool,
        now: i64,
    ) -> Result<SubscriptionSourceState> {
        let clock = OperationClock::new(now)?;
        if failure.message.len() > 4096
            || retry_at_ms.is_some_and(|t| t < now)
            || (replay_cursor
                && failure.kind != bib_types::discovery::SourceFailureKind::CursorExpired)
        {
            return Err(invalid("invalid persisted source failure or retry time"));
        }
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            let mut state = source(&conn, &lease.rule_id, name).await?;
            if state.sequence != sequence || (state.complete && state.plan.is_some()) {
                return Err(conflict("source checkpoint already completed or changed"));
            }
            if replay_cursor {
                if state.plan.is_none() {
                    return Err(invalid("cannot replay an unplanned source"));
                }
                state.cursor = None;
                state.scanned_records = 0;
                conn.execute(
                    "DELETE FROM bibliographySubscriptionCursors WHERE ruleID=?1 AND source=?2",
                    turso::params![lease.rule_id.clone(), name.to_owned()],
                )
                .await?;
            }
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or_else(|| invalid("source sequence exhausted"))?;
            runs::failed_page(&conn, lease, name, sequence, &failure).await?;
            state.last_error = Some(failure);
            state.retry_at_ms = retry_at_ms;
            state.last_page = None;
            save_source(&conn, &lease.rule_id, &state).await?;
            fence(&conn, lease, clock.now()?).await?;
            Ok(state)
        }
        .await;
        finish(tx, result).await
    }
    pub async fn open(path: &str) -> Result<Self> {
        let mut builder = turso::Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let conn = db.connect()?;
        conn.pragma_update("foreign_keys", true).await?;
        conn.pragma_update("busy_timeout", 5000).await?;
        conn.execute_batch(SCHEMA).await?;
        conn.execute(
            "INSERT OR IGNORE INTO bibliographySubscriptionMeta VALUES('limits',?1)",
            turso::params![serde_json::to_string(&SubscriptionLimits::default())?],
        )
        .await?;
        limits(&conn).await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    pub async fn limits(&self) -> Result<SubscriptionLimits> {
        limits(&*self.conn.lock().await).await
    }
    /// Lowering quotas keeps existing rules and observations intact.
    pub async fn set_limits(&self, value: SubscriptionLimits) -> Result<()> {
        value.validate()?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            if limits(&conn).await? != value {
                conn.execute(
                    "UPDATE bibliographySubscriptionMeta SET value=?1 WHERE key='limits'",
                    turso::params![serde_json::to_string(&value)?],
                )
                .await?;
                scheduling::resources_changed(&conn).await?;
            }
            Ok(())
        }
        .await;
        finish(tx, result).await
    }

    pub async fn create(&self, spec: SubscriptionSpec, now: i64) -> Result<SubscriptionRule> {
        time(now)?;
        let spec = spec.validate()?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            let policy = limits(&conn).await?;
            let mut rows = conn
                .query("SELECT COUNT(*) FROM bibliographySubscriptions", ())
                .await?;
            let count: i64 = rows
                .next()
                .await?
                .ok_or_else(|| invalid("missing rule count"))?
                .get(0)?;
            drop(rows);
            if count >= i64::from(policy.max_rules) {
                return Err(SubscriptionError::Budget("rule count".into()));
            }
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO bibliographySubscriptions VALUES(?1,1,?2,?3,NULL,NULL,0,0)",
                turso::params![id.clone(), serde_json::to_string(&spec)?, now],
            )
            .await?;
            for name in &spec.request.sources {
                conn.execute(
                    "INSERT INTO bibliographySubscriptionSources VALUES(?1,?2,?3)",
                    turso::params![
                        id.clone(),
                        name.clone(),
                        serde_json::to_string(&SubscriptionSourceState::new(name.clone()))?
                    ],
                )
                .await?;
            }
            rule(&conn, &id).await
        }
        .await;
        finish(tx, result).await
    }
    pub async fn get(&self, id: &str) -> Result<SubscriptionRule> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = rule(&conn, id).await;
        finish(tx, result).await
    }
    pub async fn list(&self) -> Result<Vec<SubscriptionRule>> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = async {
            let mut rows = conn
                .query(
                    "SELECT id FROM bibliographySubscriptions ORDER BY id LIMIT 10001",
                    (),
                )
                .await?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await? {
                ids.push(row.get::<String>(0)?);
            }
            drop(rows);
            if ids.len() > 10000 {
                return Err(SubscriptionError::Budget(
                    "rule listing exceeds supported limit".into(),
                ));
            }
            let mut result = Vec::new();
            for id in ids {
                result.push(rule(&conn, &id).await?);
            }
            Ok(result)
        }
        .await;
        finish(tx, result).await
    }
    /// Replacing any configuration fences the old owner and resets execution
    /// plans/watermarks. Seen identities and notifications are retained.
    pub async fn replace(
        &self,
        id: &str,
        version: u64,
        spec: SubscriptionSpec,
        now: i64,
    ) -> Result<SubscriptionRule> {
        time(now)?;
        let spec = spec.validate()?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let current=rule(&conn,id).await?;
            if current.version!=version{return Err(conflict("rule version changed"));}
            if version>=i64::MAX as u64{return Err(invalid("rule version exhausted"));}
            runs::cancel_running(&conn,id,now,"rule replaced",SubscriptionRunState::Cancelled).await?;
            conn.execute("UPDATE bibliographySubscriptions SET version=version+1,spec=?1,nextDue=?2,lastSuccess=NULL,owner=NULL,leaseUntil=0 WHERE id=?3",turso::params![serde_json::to_string(&spec)?,now,id.to_owned()]).await?;
            conn.execute("DELETE FROM bibliographySubscriptionSources WHERE ruleID=?1",[id]).await?;
            for name in &spec.request.sources {
                conn.execute("INSERT INTO bibliographySubscriptionSources VALUES(?1,?2,?3)",turso::params![id.to_owned(),name.clone(),serde_json::to_string(&SubscriptionSourceState::new(name.clone()))?]).await?;
            }
            rule(&conn,id).await
        }.await;
        finish(tx, result).await
    }
    pub async fn delete(&self, id: &str, version: u64) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            if rule(&conn, id).await?.version != version {
                return Err(conflict("rule version changed"));
            }
            conn.execute("DELETE FROM bibliographySubscriptions WHERE id=?1", [id])
                .await?;
            scheduling::resources_changed(&conn).await?;
            Ok(())
        }
        .await;
        finish(tx, result).await
    }
    /// Pause/resume fences the current execution without discarding its frozen
    /// window or cursor. A subsequent owner continues the same checkpoint.
    pub async fn set_enabled(
        &self,
        id: &str,
        version: u64,
        enabled: bool,
        now: i64,
    ) -> Result<SubscriptionRule> {
        time(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let mut current=rule(&conn,id).await?;
            if current.version!=version{return Err(conflict("rule version changed"));}
            if version>=i64::MAX as u64{return Err(invalid("rule version exhausted"));}
            runs::cancel_running(&conn,id,now,"rule paused or resumed",SubscriptionRunState::Cancelled).await?;
            current.spec.enabled=enabled;
            conn.execute("UPDATE bibliographySubscriptions SET version=version+1,spec=?1,nextDue=?2,owner=NULL,leaseUntil=0 WHERE id=?3",turso::params![serde_json::to_string(&current.spec)?,now,id.to_owned()]).await?;
            rule(&conn,id).await
        }.await;
        finish(tx, result).await
    }
    /// Release a partial/failed round. This deliberately cannot mark a rule
    /// successful or advance any source watermark.
    pub async fn release(&self, lease: &SubscriptionLease, now: i64, delay_ms: u32) -> Result<()> {
        let clock = OperationClock::new(now)?;
        if delay_ms > 2_678_400_000 {
            return Err(invalid("retry delay exceeds 31 days"));
        }
        let due = now
            .checked_add(i64::from(delay_ms))
            .ok_or_else(|| invalid("retry time overflow"))?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let deadline=fence(&conn,lease,clock.now()?).await?;
            conn.execute("UPDATE bibliographySubscriptions SET owner=NULL,leaseUntil=0,nextDue=?1 WHERE id=?2",turso::params![due,lease.rule_id.clone()]).await?;
            if clock.now()? >= deadline{return Err(conflict("lease expired while releasing execution"));}
            Ok(())
        }.await;
        finish(tx, result).await
    }
    /// Manual is explicit user intent and may check a paused rule. Scheduled
    /// claims require an enabled, due rule. Both paths compete for one lease.
    pub async fn claim(
        &self,
        id: &str,
        owner: &str,
        now: i64,
        ttl_ms: u32,
        manual: bool,
    ) -> Result<SubscriptionLease> {
        let clock = OperationClock::new(now)?;
        expiry(now, ttl_ms)?;
        if uuid::Uuid::parse_str(owner).is_err() {
            return Err(invalid("invalid worker identity"));
        }
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let current=rule(&conn,id).await?;
            let now=clock.now()?;let expires_ms=expiry(now,ttl_ms)?;
            if current.resource_wait.is_some(){return Err(conflict("rule is waiting for subscription resources to change"));}
            if !manual && (!current.spec.enabled || (current.next_due_ms>now && !scheduling::resource_retry_ready(&conn,id).await?)){return Err(conflict("rule is paused or not due"));}
            let changed=conn.execute("UPDATE bibliographySubscriptions SET owner=?1,epoch=epoch+1,leaseUntil=?2 WHERE id=?3 AND version=?4 AND leaseUntil<=?5 AND epoch<9223372036854775807",turso::params![owner.to_owned(),expires_ms,id.to_owned(),current.version as i64,now]).await?;
            if changed!=1{return Err(conflict("rule has a live owner or exhausted lease epochs"));}
            conn.execute("DELETE FROM bibliographySubscriptionResourceWait WHERE ruleID=?1",[id]).await?;
            let mut rows=conn.query("SELECT epoch FROM bibliographySubscriptions WHERE id=?1",[id]).await?;
            let epoch:i64=rows.next().await?.ok_or(SubscriptionError::NotFound)?.get(0)?;
            if clock.now()? >= expires_ms{return Err(conflict("lease expired while claiming execution"));}
            Ok(SubscriptionLease{rule_id:id.into(),version:current.version,owner:owner.into(),epoch:epoch as u64,expires_ms,run_id:None})
        }.await;
        finish(tx, result).await
    }
    pub async fn heartbeat(
        &self,
        lease: &SubscriptionLease,
        now: i64,
        ttl_ms: u32,
    ) -> Result<SubscriptionLease> {
        let clock = OperationClock::new(now)?;
        expiry(now, ttl_ms)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            let original_deadline = fence(&conn, lease, clock.now()?).await?;
            let expires_ms = expiry(clock.now()?, ttl_ms)?;
            conn.execute(
                "UPDATE bibliographySubscriptions SET leaseUntil=MAX(leaseUntil,?1) WHERE id=?2",
                turso::params![expires_ms, lease.rule_id.clone()],
            )
            .await?;
            let mut renewed = lease.clone();
            renewed.expires_ms = lease.expires_ms.max(expires_ms);
            if clock.now()? >= original_deadline {
                return Err(conflict("lease expired while renewing execution"));
            }
            renewed.expires_ms = fence(&conn, &renewed, clock.now()?).await?;
            Ok(renewed)
        }
        .await;
        finish(tx, result).await
    }
    /// Persist the actual closed window/plan before requesting its first page.
    /// An unfinished plan cannot be replaced by a later moving time window.
    pub async fn prepare_source(
        &self,
        lease: &SubscriptionLease,
        name: &str,
        sequence: u64,
        plan: SourcePlan,
        window: Option<SubscriptionWindow>,
        now: i64,
    ) -> Result<SubscriptionSourceState> {
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            let current = rule(&conn, &lease.rule_id).await?;
            let mut state = source(&conn, &lease.rule_id, name).await?;
            if state.sequence != sequence || !state.complete {
                return Err(conflict("source has an unfinished or changed checkpoint"));
            }
            if plan.source != name
                || plan.page_size != current.spec.request.page_size
                || plan.fingerprint.len() != 64
                || !plan.fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
                || serde_json::to_vec(&plan)?.len() > 65536
            {
                return Err(invalid("invalid source plan"));
            }
            if plan != crate::subscription_plan::plan(&current.spec.request, name, window.as_ref())?
            {
                return Err(invalid(
                    "source plan differs from the saved rule and declared window",
                ));
            }
            if let Some(w) = &window {
                if w.from_ms < 0
                    || w.through_ms < w.from_ms
                    || w.through_ms > now
                    || !matches!(
                        (name, w.kind),
                        ("pubmed", SubscriptionWindowKind::PubmedCreatedOrRevised)
                            | ("crossref", SubscriptionWindowKind::CrossrefIndexed)
                    )
                {
                    return Err(invalid("invalid source time window"));
                }
            }
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or_else(|| invalid("source sequence exhausted"))?;
            state.plan = Some(plan);
            state.window = window;
            state.cursor = None;
            state.complete = false;
            state.scanned_records = 0;
            state.retry_at_ms = None;
            state.last_error = None;
            state.last_page = None;
            conn.execute(
                "DELETE FROM bibliographySubscriptionCursors WHERE ruleID=?1 AND source=?2",
                turso::params![lease.rule_id.clone(), name.to_owned()],
            )
            .await?;
            save_source(&conn, &lease.rule_id, &state).await?;
            fence(&conn, lease, clock.now()?).await?;
            Ok(state)
        }
        .await;
        finish(tx, result).await
    }
}
