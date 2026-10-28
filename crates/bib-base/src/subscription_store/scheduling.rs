//! Persistent resource waits and small due-rule queries for the scheduler.
use super::*;
pub(super) async fn resource_epoch(conn: &Connection) -> Result<i64> {
    let mut rows=conn.query("SELECT CAST(value AS INTEGER) FROM bibliographySubscriptionMeta WHERE key='resourceEpoch'",()).await?;
    let epoch: i64 = rows
        .next()
        .await?
        .ok_or_else(|| invalid("missing resource epoch"))?
        .get(0)?;
    if epoch < 0 {
        return Err(invalid("negative resource epoch"));
    }
    Ok(epoch)
}
pub(super) async fn resources_changed(conn: &Connection) -> Result<()> {
    let epoch = resource_epoch(conn)
        .await?
        .checked_add(1)
        .ok_or_else(|| invalid("resource epoch exhausted"))?;
    conn.execute(
        "UPDATE bibliographySubscriptionMeta SET value=?1 WHERE key='resourceEpoch'",
        [epoch.to_string()],
    )
    .await?;
    Ok(())
}
pub(super) async fn resource_wait(
    conn: &Connection,
    id: &str,
) -> Result<Option<SubscriptionResourceWait>> {
    let epoch = resource_epoch(conn).await?;
    let mut rows=conn.query("SELECT since,reason,resourceEpoch FROM bibliographySubscriptionResourceWait WHERE ruleID=?1 AND resourceEpoch=?2",turso::params![id.to_owned(),epoch]).await?;
    rows.next()
        .await?
        .map(|r| {
            Ok(SubscriptionResourceWait {
                since_ms: r.get(0)?,
                reason: r.get(1)?,
                observed_epoch: r.get(2)?,
            })
        })
        .transpose()
}
pub(super) async fn resource_retry_ready(conn: &Connection, id: &str) -> Result<bool> {
    let epoch = resource_epoch(conn).await?;
    let mut rows=conn.query("SELECT 1 FROM bibliographySubscriptionResourceWait WHERE ruleID=?1 AND resourceEpoch<?2",turso::params![id.to_owned(),epoch]).await?;
    Ok(rows.next().await?.is_some())
}
impl SubscriptionStore {
    pub async fn resource_epoch(&self) -> Result<i64> {
        resource_epoch(&*self.conn.lock().await).await
    }
    /// Capture the resource epoch BEFORE the operation that might fail. A
    /// concurrent quota change/deletion must not be missed by recording a later
    /// epoch after the failing transaction has rolled back.
    pub async fn wait_for_resources(
        &self,
        lease: &SubscriptionLease,
        reason: &str,
        observed_epoch: i64,
        now: i64,
    ) -> Result<()> {
        if reason.is_empty() || reason.len() > 512 || observed_epoch < 0 {
            return Err(invalid("invalid resource wait"));
        }
        let clock = OperationClock::new(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            fence(&conn,lease,clock.now()?).await?;
            if observed_epoch>resource_epoch(&conn).await? {return Err(invalid("future resource epoch"));}
            conn.execute("INSERT INTO bibliographySubscriptionResourceWait VALUES(?1,?2,?3,?4) ON CONFLICT(ruleID) DO UPDATE SET since=excluded.since,reason=excluded.reason,resourceEpoch=excluded.resourceEpoch",turso::params![lease.rule_id.clone(),now,reason.to_owned(),observed_epoch]).await?;
            fence(&conn,lease,clock.now()?).await?;
            Ok(())
        }.await;
        finish(tx, result).await
    }
    /// JSON enabled filtering and resource waits occur before LIMIT; paused or
    /// blocked rules cannot occupy every candidate slot and starve due rules.
    pub async fn due(&self, now: i64, limit: u16) -> Result<Vec<String>> {
        time(now)?;
        if !(1..=128).contains(&limit) {
            return Err(invalid("invalid due candidate limit"));
        }
        let conn = self.conn.lock().await;
        let mut rows=conn.query("SELECT s.id FROM bibliographySubscriptions s WHERE (s.nextDue<=?1 OR EXISTS(SELECT 1 FROM bibliographySubscriptionResourceWait retry WHERE retry.ruleID=s.id AND retry.resourceEpoch<(SELECT CAST(value AS INTEGER) FROM bibliographySubscriptionMeta WHERE key='resourceEpoch'))) AND s.leaseUntil<=?1 AND json_extract(s.spec,'$.enabled')=1 AND NOT EXISTS(SELECT 1 FROM bibliographySubscriptionResourceWait w WHERE w.ruleID=s.id AND w.resourceEpoch=(SELECT CAST(value AS INTEGER) FROM bibliographySubscriptionMeta WHERE key='resourceEpoch')) ORDER BY s.nextDue,s.id LIMIT ?2",turso::params![now,i64::from(limit)]).await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }
    /// Idempotently cancel the named run. Cancelling an old terminal attempt
    /// never releases a newer run's lease or changes its source checkpoint.
    pub async fn cancel_run(&self, id: &str, now: i64) -> Result<SubscriptionRun> {
        time(now)?;
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async {
            let mut run=runs::load(&conn,id).await?;
            if run.state!=SubscriptionRunState::Running {return Ok(run);}
            run.state=SubscriptionRunState::Cancelled;run.reason=Some("cancelled explicitly".into());run.finished_ms=Some(now);run.reserved_records=0;
            for s in &mut run.sources {s.pending=None;}
            runs::write(&conn,&run,false).await?;
            let due=now.checked_add(i64::from(run.spec.interval_seconds)*1000).ok_or_else(||invalid("schedule overflow"))?;
            conn.execute("UPDATE bibliographySubscriptions SET owner=NULL,leaseUntil=0,nextDue=?1 WHERE id=?2 AND epoch=?3 AND version=?4",turso::params![due,run.rule_id.clone(),run.lease_epoch as i64,run.rule_version as i64]).await?;
            Ok(run)
        }.await;
        finish(tx, result).await
    }
}
