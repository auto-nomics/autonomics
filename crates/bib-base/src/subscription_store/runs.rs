//! Persistent attempts and per-source scans. Request reservations precede I/O;
//! page settlement is called by the same transaction as inbox/checkpoint writes.
use super::*;
use bib_types::discovery::SourceFailure;
const SETTLEMENT_RESERVE: usize = 65_536;
pub(super) async fn load(conn: &Connection, id: &str) -> Result<SubscriptionRun> {
    let mut rows = conn
        .query(
            "SELECT value FROM bibliographySubscriptionRuns WHERE id=?1",
            [id],
        )
        .await?;
    Ok(serde_json::from_str(
        &rows
            .next()
            .await?
            .ok_or(SubscriptionError::NotFound)?
            .get::<String>(0)?,
    )?)
}
pub(super) async fn write(conn: &Connection, run: &SubscriptionRun, allocate: bool) -> Result<()> {
    let policy = limits(conn).await?;
    let value = serde_json::to_string(run)?;
    let mut rows = conn
        .query(
            "SELECT bytes FROM bibliographySubscriptionRuns WHERE id=?1",
            [run.id.as_str()],
        )
        .await?;
    let old = rows.next().await?.map(|r| r.get::<i64>(0)).transpose()?;
    drop(rows);
    let charge = if allocate {
        if value.len() > policy.max_run_bytes as usize {
            return Err(SubscriptionError::RunBudget(
                "execution history payload".into(),
            ));
        }
        (value.len() + SETTLEMENT_RESERVE).max(old.unwrap_or(0) as usize)
    } else {
        let old = old.ok_or(SubscriptionError::NotFound)?;
        if value.len() > old as usize {
            return Err(SubscriptionError::Budget(
                "run settlement reservation".into(),
            ));
        }
        if run.state == SubscriptionRunState::Running {
            old as usize
        } else {
            value.len()
        }
    };
    conn.execute("INSERT INTO bibliographySubscriptionRuns(id,ruleID,ruleVersion,leaseEpoch,started,finished,state,pagesStarted,pagesCommitted,records,notifications,value,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(id) DO UPDATE SET finished=excluded.finished,state=excluded.state,pagesStarted=excluded.pagesStarted,pagesCommitted=excluded.pagesCommitted,records=excluded.records,notifications=excluded.notifications,value=excluded.value,bytes=excluded.bytes",turso::params![run.id.clone(),run.rule_id.clone(),run.rule_version as i64,run.lease_epoch as i64,run.started_ms,run.finished_ms,run.state.as_str(),i64::from(run.pages_started),i64::from(run.pages_committed),i64::from(run.records),i64::from(run.new_notifications),value,charge as i64]).await?;
    if allocate {
        let mut rows=conn.query("SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM bibliographySubscriptionRuns INDEXED BY bibliographySubscriptionRuns_usage",()).await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| invalid("missing run quota accounting"))?;
        if row.get::<i64>(0)? > i64::from(policy.max_runs)
            || row.get::<i64>(1)? as u64 > policy.max_history_bytes
        {
            return Err(SubscriptionError::Budget(
                "retained execution history".into(),
            ));
        }
    }
    Ok(())
}
fn strategy(spec: &SubscriptionSpec, name: &str) -> SubscriptionScanStrategy {
    match name {
        "pubmed" => SubscriptionScanStrategy::PubmedCreatedOrRevised,
        "crossref" => {
            let replay = crate::discovery_plan::plan(name, &spec.request)
                .ok()
                .is_some_and(|p| {
                    p.parameters.contains_key("doi")
                        || p.parameters.get("filter").is_some_and(|v| {
                            v.split(',').any(|f| {
                                matches!(
                                    f.split_once(':').map(|(k, _)| k),
                                    Some("from-index-date" | "until-index-date")
                                )
                            })
                        })
                });
            if replay {
                SubscriptionScanStrategy::BoundedReplay
            } else {
                SubscriptionScanStrategy::CrossrefIndexed
            }
        }
        _ => SubscriptionScanStrategy::BoundedReplay,
    }
}
async fn running(conn: &Connection, lease: &SubscriptionLease) -> Result<SubscriptionRun> {
    let id = lease
        .run_id
        .as_ref()
        .ok_or_else(|| invalid("a persistent run lease is required"))?;
    let run = load(conn, id).await?;
    if run.rule_id != lease.rule_id
        || run.rule_version != lease.version
        || run.lease_epoch != lease.epoch
        || run.state != SubscriptionRunState::Running
    {
        return Err(conflict("execution finished or ownership changed"));
    }
    Ok(run)
}
pub(super) async fn cancel_running(
    conn: &Connection,
    id: &str,
    now: i64,
    reason: &str,
    phase: SubscriptionRunState,
) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT id FROM bibliographySubscriptionRuns WHERE ruleID=?1 AND state='running'",
            [id],
        )
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<String>(0)?);
    }
    drop(rows);
    for id in ids {
        let mut run = load(conn, &id).await?;
        run.state = phase;
        run.finished_ms = Some(now);
        run.reason = Some(reason.into());
        run.reserved_records = 0;
        for s in &mut run.sources {
            s.pending = None;
        }
        write(conn, &run, false).await?;
    }
    Ok(())
}
impl SubscriptionStore {
    pub async fn run(&self, id: &str) -> Result<SubscriptionRun> {
        load(&*self.conn.lock().await, id).await
    }
    pub async fn begin_run(
        &self,
        lease: &SubscriptionLease,
        id: &str,
        manual: bool,
        now: i64,
    ) -> Result<(SubscriptionLease, SubscriptionRun)> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(invalid("invalid run UUID"));
        }
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            match load(&conn, id).await {
                Ok(saved) => {
                    if saved.rule_id != lease.rule_id
                        || saved.rule_version != lease.version
                        || saved.lease_epoch != lease.epoch
                    {
                        return Err(conflict("run UUID already belongs to another execution"));
                    }
                    let mut bound = lease.clone();
                    bound.run_id = Some(id.into());
                    return Ok((bound, saved));
                }
                Err(SubscriptionError::NotFound) => {}
                Err(error) => return Err(error),
            }
            let mut rows = conn
                .query(
                    "SELECT 1 FROM bibliographySubscriptionRuns WHERE ruleID=?1 AND leaseEpoch=?2",
                    turso::params![lease.rule_id.clone(), lease.epoch as i64],
                )
                .await?;
            if rows.next().await?.is_some() {
                return Err(conflict("lease already has an execution"));
            }
            drop(rows);
            cancel_running(
                &conn,
                &lease.rule_id,
                now,
                "lease taken over after interruption",
                SubscriptionRunState::Interrupted,
            )
            .await?;
            let spec = rule(&conn, &lease.rule_id).await?.spec;
            let sources = spec
                .request
                .sources
                .iter()
                .map(|name| {
                    let strategy = strategy(&spec, name);
                    let unit = match strategy {
                        SubscriptionScanStrategy::PubmedCreatedOrRevised => {
                            Some(crate::subscription_plan::DAY_MS)
                        }
                        SubscriptionScanStrategy::CrossrefIndexed => Some(1000),
                        SubscriptionScanStrategy::BoundedReplay => None,
                    };
                    SubscriptionRunSource {
                        source: name.clone(),
                        strategy,
                        target_ms: unit.map(|unit| now / unit * unit - 1),
                        scan_id: None,
                        done: false,
                        pending: None,
                        windows: Vec::new(),
                        error: None,
                    }
                })
                .collect();
            let run = SubscriptionRun {
                id: id.into(),
                rule_id: lease.rule_id.clone(),
                rule_version: lease.version,
                lease_epoch: lease.epoch,
                deadline_ms: now
                    .checked_add(spec.run_timeout_ms as i64)
                    .ok_or_else(|| invalid("run deadline overflow"))?,
                spec,
                manual,
                started_ms: now,
                finished_ms: None,
                state: SubscriptionRunState::Running,
                reason: None,
                pages_started: 0,
                pages_committed: 0,
                records: 0,
                new_notifications: 0,
                reserved_records: 0,
                sources,
            };
            write(&conn, &run, true).await?;
            fence(&conn, lease, clock.now()?).await?;
            let mut bound = lease.clone();
            bound.run_id = Some(id.into());
            Ok((bound, run))
        }
        .await;
        finish(tx, result).await
    }
    /// Continue a source's frozen scan, independently of the other sources.
    pub async fn prepare_run_source(
        &self,
        lease: &SubscriptionLease,
        name: &str,
        now: i64,
    ) -> Result<Option<SubscriptionSourceState>> {
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            let mut run = running(&conn, lease).await?;
            if clock.now()? >= run.deadline_ms {
                return Err(SubscriptionError::RunBudget("time".into()));
            }
            let position = run
                .sources
                .iter()
                .position(|s| s.source == name)
                .ok_or_else(|| invalid("source outside execution"))?;
            if run.sources[position].done {
                return Ok(None);
            }
            let mut state = source(&conn, &lease.rule_id, name).await?;
            if state.scan.as_ref().is_none_or(|s| s.done) {
                let target = run.sources[position].target_ms;
                let next = target.map(|target| {
                    state
                        .watermark_ms
                        .map_or_else(
                            || {
                                target
                                    .saturating_sub(
                                        i64::from(run.spec.initial_days)
                                            * crate::subscription_plan::DAY_MS,
                                    )
                                    .saturating_add(1)
                            },
                            |watermark| {
                                watermark.saturating_add(1).saturating_sub(
                                    i64::from(run.spec.overlap_days)
                                        * crate::subscription_plan::DAY_MS,
                                )
                            },
                        )
                        .max(0)
                });
                state.scan = Some(SubscriptionScan {
                    id: uuid::Uuid::new_v4().to_string(),
                    strategy: run.sources[position].strategy,
                    started_ms: now,
                    target_ms: target,
                    next_from_ms: next,
                    done: target.zip(next).is_some_and(|(t, n)| n > t),
                });
            }
            run.sources[position].error = state.last_error.clone();
            let scan = state.scan.as_ref().unwrap();
            run.sources[position].scan_id = Some(scan.id.clone());
            if scan.done {
                run.sources[position].done = true;
                save_source(&conn, &lease.rule_id, &state).await?;
                write(&conn, &run, false).await?;
                fence(&conn, lease, clock.now()?).await?;
                return Ok(None);
            }
            if !state.complete && state.plan.is_some() {
                write(&conn, &run, false).await?;
                fence(&conn, lease, clock.now()?).await?;
                return Ok(Some(state));
            }
            let window = match scan.strategy {
                SubscriptionScanStrategy::BoundedReplay => None,
                strategy => {
                    let from = scan
                        .next_from_ms
                        .ok_or_else(|| invalid("missing scan start"))?;
                    let through = (from
                        + i64::from(run.spec.window_days) * crate::subscription_plan::DAY_MS
                        - 1)
                    .min(
                        scan.target_ms
                            .ok_or_else(|| invalid("missing scan target"))?,
                    );
                    Some(SubscriptionWindow {
                        kind: if strategy == SubscriptionScanStrategy::PubmedCreatedOrRevised {
                            SubscriptionWindowKind::PubmedCreatedOrRevised
                        } else {
                            SubscriptionWindowKind::CrossrefIndexed
                        },
                        from_ms: from,
                        through_ms: through,
                    })
                }
            };
            let plan = crate::subscription_plan::plan(&run.spec.request, name, window.as_ref())?;
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or_else(|| invalid("source sequence exhausted"))?;
            state.window = window;
            state.plan = Some(plan);
            state.cursor = None;
            state.complete = false;
            state.scanned_records = 0;
            state.last_page = None;
            // Retry state remains visible until a successful page; the caller
            // cannot bypass a persisted Retry-After by preparing the same plan.
            conn.execute(
                "DELETE FROM bibliographySubscriptionCursors WHERE ruleID=?1 AND source=?2",
                turso::params![lease.rule_id.clone(), name.to_owned()],
            )
            .await?;
            save_source(&conn, &lease.rule_id, &state).await?;
            write(&conn, &run, false).await?;
            fence(&conn, lease, clock.now()?).await?;
            Ok(Some(state))
        }
        .await;
        finish(tx, result).await
    }
    pub async fn reserve_run_page(
        &self,
        lease: &SubscriptionLease,
        name: &str,
        sequence: u64,
        now: i64,
    ) -> Result<SubscriptionPendingPage> {
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            let mut run = running(&conn, lease).await?;
            if clock.now()? >= run.deadline_ms {
                return Err(SubscriptionError::RunBudget("time".into()));
            }
            let state = source(&conn, &lease.rule_id, name).await?;
            let plan = state
                .plan
                .as_ref()
                .ok_or_else(|| conflict("source is not prepared"))?;
            if state.sequence != sequence
                || state.complete
                || state
                    .retry_at_ms
                    .is_some_and(|t| t > clock.now().unwrap_or(i64::MAX))
            {
                return Err(conflict("source checkpoint is not eligible"));
            }
            if state.last_error.as_ref().is_some_and(|e| !e.retryable) && !run.manual {
                return Err(conflict("source requires explicit retry or a rule change"));
            }
            let s = run
                .sources
                .iter_mut()
                .find(|s| s.source == name)
                .ok_or_else(|| invalid("source outside execution"))?;
            if s.pending.is_some() {
                return Err(conflict("source already has an in-flight page"));
            }
            if run.pages_started >= u32::from(run.spec.max_pages_per_run)
                || run.records + run.reserved_records + u32::from(plan.page_size)
                    > run.spec.max_records_per_run
            {
                return Err(SubscriptionError::RunBudget(
                    "pages or whole-page records".into(),
                ));
            }
            let pending = SubscriptionPendingPage {
                sequence,
                cursor: state.cursor.clone(),
                page_size: plan.page_size,
            };
            if s.windows.last().is_none_or(|w| {
                w.complete || w.plan.fingerprint != plan.fingerprint || w.window != state.window
            }) {
                s.windows.push(SubscriptionWindowExecution {
                    plan: plan.clone(),
                    window: state.window.clone(),
                    initial_cursor: state.cursor.clone(),
                    pages_started: 0,
                    pages_committed: 0,
                    records: 0,
                    complete: false,
                });
            }
            s.windows.last_mut().unwrap().pages_started += 1;
            s.pending = Some(pending.clone());
            s.error = None;
            run.pages_started += 1;
            run.reserved_records += u32::from(plan.page_size);
            write(&conn, &run, true).await?;
            fence(&conn, lease, clock.now()?).await?;
            Ok(pending)
        }
        .await;
        finish(tx, result).await
    }
    pub async fn finish_run(
        &self,
        lease: &SubscriptionLease,
        state: SubscriptionRunState,
        reason: Option<String>,
        now: i64,
    ) -> Result<SubscriptionRun> {
        if state == SubscriptionRunState::Running || reason.as_ref().is_some_and(|s| s.len() > 512)
        {
            return Err(invalid("invalid run termination"));
        }
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let deadline=fence(&conn,lease,clock.now()?).await?;let mut run=running(&conn,lease).await?;
            if state==SubscriptionRunState::Complete && run.sources.iter().any(|s|!s.done || s.error.is_some() || s.pending.is_some()){return Err(conflict("not every source completed its frozen target"));}
            run.state=state;run.reason=reason;run.finished_ms=Some(now);run.reserved_records=0;for s in &mut run.sources{s.pending=None;}write(&conn,&run,false).await?;
            let due=now.checked_add(i64::from(run.spec.interval_seconds)*1000).ok_or_else(||invalid("schedule overflow"))?;
            conn.execute("UPDATE bibliographySubscriptions SET owner=NULL,leaseUntil=0,nextDue=?1,lastSuccess=CASE WHEN ?2=1 THEN ?3 ELSE lastSuccess END WHERE id=?4",turso::params![due,i64::from(state==SubscriptionRunState::Complete),now,lease.rule_id.clone()]).await?;
            if clock.now()?>=deadline{return Err(conflict("lease expired while finishing execution"));}Ok(run)
        }.await;
        finish(tx, result).await
    }
    pub async fn runs(
        &self,
        rule_id: &str,
        before: Option<i64>,
        limit: u16,
    ) -> Result<(Vec<SubscriptionRunSummary>, Option<i64>)> {
        if !(1..=100).contains(&limit) || before.is_some_and(|n| n <= 0) {
            return Err(invalid("invalid execution history page"));
        }
        let conn = self.conn.lock().await;
        let mut rows=conn.query("SELECT seq,id,ruleVersion,started,finished,state,pagesStarted,pagesCommitted,records,notifications FROM bibliographySubscriptionRuns WHERE ruleID=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3",turso::params![rule_id.to_owned(),before.unwrap_or(i64::MAX),i64::from(limit)+1]).await?;
        let mut result = Vec::new();
        let mut last = None;
        let mut more = false;
        while let Some(row) = rows.next().await? {
            if result.len() == usize::from(limit) {
                more = true;
                break;
            }
            last = Some(row.get(0)?);
            let phase: String = row.get(5)?;
            result.push(SubscriptionRunSummary {
                id: row.get(1)?,
                rule_id: rule_id.into(),
                rule_version: row.get::<i64>(2)? as u64,
                started_ms: row.get(3)?,
                finished_ms: row.get(4)?,
                state: serde_json::from_str(&serde_json::to_string(&phase)?)?,
                pages_started: row.get::<i64>(6)? as u32,
                pages_committed: row.get::<i64>(7)? as u32,
                records: row.get::<i64>(8)? as u32,
                new_notifications: row.get::<i64>(9)? as u32,
            });
        }
        Ok((result, if more { last } else { None }))
    }
    pub async fn delete_run(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            if load(&conn, id).await?.state == SubscriptionRunState::Running {
                return Err(conflict("cannot delete an active execution"));
            }
            conn.execute("DELETE FROM bibliographySubscriptionRuns WHERE id=?1", [id])
                .await?;
            scheduling::resources_changed(&conn).await?;
            Ok(())
        }
        .await;
        finish(tx, result).await
    }
}

pub(super) async fn committed_page(
    conn: &Connection,
    lease: &SubscriptionLease,
    state: &mut SubscriptionSourceState,
    receipt: &SubscriptionPageReceipt,
    cursor: Option<&str>,
    count: usize,
    now: i64,
) -> Result<()> {
    if lease.run_id.is_none() {
        return Ok(());
    }
    let mut run = running(conn, lease).await?;
    if now >= run.deadline_ms {
        return Err(SubscriptionError::RunBudget("time".into()));
    }
    let s = run
        .sources
        .iter_mut()
        .find(|s| s.source == state.source)
        .ok_or_else(|| invalid("source outside execution"))?;
    let pending = s
        .pending
        .take()
        .ok_or_else(|| conflict("source page was not reserved"))?;
    if pending.sequence != receipt.expected_sequence
        || pending.cursor.as_deref() != cursor
        || count > usize::from(pending.page_size)
    {
        return Err(conflict("reserved source page differs from response"));
    }
    run.reserved_records -= u32::from(pending.page_size);
    run.records += count as u32;
    run.pages_committed += 1;
    run.new_notifications += receipt.new_notifications as u32;
    let w = s
        .windows
        .last_mut()
        .ok_or_else(|| invalid("missing execution window"))?;
    w.pages_committed += 1;
    w.records += count as u32;
    w.complete = state.complete;
    s.error = None;
    let scan = state
        .scan
        .as_mut()
        .ok_or_else(|| conflict("source has no frozen scan"))?;
    if s.scan_id.as_ref() != Some(&scan.id) {
        return Err(conflict("source scan changed"));
    }
    if state.complete {
        if let Some(window) = &state.window {
            if Some(window.from_ms) != scan.next_from_ms
                || scan.target_ms.is_none_or(|t| window.through_ms > t)
            {
                return Err(conflict("window skipped a scan checkpoint"));
            }
            scan.next_from_ms = Some(
                window
                    .through_ms
                    .checked_add(1)
                    .ok_or_else(|| invalid("scan time overflow"))?,
            );
            scan.done = scan.next_from_ms > scan.target_ms;
        } else {
            scan.done = true;
        }
        s.done = scan.done
            && match (s.target_ms, scan.target_ms) {
                (None, None) => true,
                (Some(t), Some(actual)) => actual >= t,
                _ => false,
            };
    }
    write(conn, &run, false).await
}
pub(super) async fn failed_page(
    conn: &Connection,
    lease: &SubscriptionLease,
    name: &str,
    sequence: u64,
    failure: &SourceFailure,
) -> Result<()> {
    if lease.run_id.is_none() {
        return Ok(());
    }
    let mut run = running(conn, lease).await?;
    let s = run
        .sources
        .iter_mut()
        .find(|s| s.source == name)
        .ok_or_else(|| invalid("source outside execution"))?;
    if let Some(p) = s.pending.take() {
        if p.sequence != sequence {
            return Err(conflict("failed page reservation changed"));
        }
        run.reserved_records -= u32::from(p.page_size);
    }
    s.error = Some(failure.clone());
    write(conn, &run, false).await
}
