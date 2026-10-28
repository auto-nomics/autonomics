//! A page's seen identities, notifications and checkpoint commit together.
use super::*;
use bib_types::{
    Article,
    discovery::{DiscoveryRecord, SourcePage},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};

fn identity(article: &Article) -> Article {
    let mut result = Article::new(article.id.clone(), String::new());
    result.identifiers = article.identifiers.clone();
    result.source = article.source;
    result.pub_types = article.pub_types.clone();
    result.journal = article.journal.clone();
    result
}
async fn inbox(conn: &Connection, id: &str, group: &str) -> Result<Option<SubscriptionInboxItem>> {
    let mut rows = conn
        .query(
            "SELECT value FROM bibliographySubscriptionInbox WHERE ruleID=?1 AND groupID=?2",
            turso::params![id.to_owned(), group.to_owned()],
        )
        .await?;
    rows.next()
        .await?
        .map(|r| -> Result<_> { Ok(serde_json::from_str(&r.get::<String>(0)?)?) })
        .transpose()
}
async fn put_inbox(
    conn: &Connection,
    item: &SubscriptionInboxItem,
    policy: &SubscriptionLimits,
) -> Result<()> {
    let value = serde_json::to_string(item)?;
    if value.len() > policy.max_page_bytes as usize || item.records.len() > 512 {
        return Err(SubscriptionError::Budget(
            "notification group payload".into(),
        ));
    }
    conn.execute("INSERT INTO bibliographySubscriptionInbox(id,ruleID,groupID,value,bytes) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(ruleID,groupID) DO UPDATE SET value=excluded.value,bytes=excluded.bytes",turso::params![item.id.clone(),item.rule_id.clone(),item.group_id.clone(),value.clone(),value.len() as i64]).await?;
    Ok(())
}
async fn members(conn: &Connection, id: &str, group: &str) -> Result<Vec<Article>> {
    let mut rows=conn.query("SELECT identity FROM bibliographySubscriptionSeen WHERE ruleID=?1 AND groupID=?2 LIMIT 513",turso::params![id.to_owned(),group.to_owned()]).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(serde_json::from_str(&row.get::<String>(0)?)?);
    }
    if result.len() > 512 {
        return Err(SubscriptionError::Budget(
            "identity group exceeds 512 source records".into(),
        ));
    }
    Ok(result)
}
async fn add_keys(conn: &Connection, id: &str, source: &str, article: &Article) -> Result<()> {
    for key in crate::discovery_merge::identity_keys(article) {
        conn.execute(
            "INSERT OR IGNORE INTO bibliographySubscriptionKeys VALUES(?1,?2,?3,?4)",
            turso::params![id.to_owned(), source.to_owned(), article.id.clone(), key],
        )
        .await?;
    }
    Ok(())
}
async fn enforce_budgets(conn: &Connection, policy: &SubscriptionLimits) -> Result<()> {
    for (sql, max_count, max_bytes) in [
        (
            "SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM bibliographySubscriptionSeen",
            policy.max_seen,
            policy.max_seen_bytes,
        ),
        (
            "SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM bibliographySubscriptionInbox",
            policy.max_inbox,
            policy.max_inbox_bytes,
        ),
    ] {
        let mut rows = conn.query(sql, ()).await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| invalid("missing quota count"))?;
        let count: i64 = row.get(0)?;
        let bytes: i64 = row.get(1)?;
        if count < 0 || bytes < 0 {
            return Err(invalid("negative retained resource accounting"));
        }
        if count > i64::from(max_count) || bytes as u64 > max_bytes {
            return Err(SubscriptionError::Budget(
                "retained seen identities or inbox payload".into(),
            ));
        }
    }
    Ok(())
}

/// Returns true for a newly observed source identity; false for replay.
async fn observe(
    conn: &Connection,
    lease: &SubscriptionLease,
    record: DiscoveryRecord,
    now: i64,
    policy: &SubscriptionLimits,
) -> Result<(bool, bool)> {
    let id = &lease.rule_id;
    let mut rows=conn.query("SELECT groupID,identity FROM bibliographySubscriptionSeen WHERE ruleID=?1 AND source=?2 AND recordID=?3",turso::params![id.clone(),record.source.clone(),record.article.id.clone()]).await?;
    let existing = rows
        .next()
        .await?
        .map(|row| -> Result<_> {
            Ok((
                row.get::<String>(0)?,
                serde_json::from_str::<Article>(&row.get::<String>(1)?)?,
            ))
        })
        .transpose()?;
    drop(rows);
    if let Some((group, mut old)) = existing {
        if !members(conn, id, &group)
            .await?
            .iter()
            .all(|a| crate::discovery_merge::compatible(a, &record.article))
        {
            return Err(invalid(
                "an existing source identity changed publication identity",
            ));
        }
        for key in &record.article.identifiers {
            if !old.identifiers.contains(key) {
                old.identifiers.push(key.clone());
            }
        }
        for value in &record.article.pub_types {
            if !old.pub_types.contains(value) {
                old.pub_types.push(value.clone());
            }
        }
        if crate::discovery_merge::preprint(&record.article)
            && !crate::discovery_merge::preprint(&old)
        {
            old.pub_types.push("preprint".into());
        }
        if crate::discovery_merge::published(&record.article)
            && !crate::discovery_merge::published(&old)
        {
            old.pub_types.push("journal article".into());
        }
        if old.journal.is_none() {
            old.journal = record.article.journal.clone();
        }
        let value = serde_json::to_string(&old)?;
        let bytes = value.len()
            + serde_json::to_vec(&crate::discovery_merge::identity_keys(&old))?.len()
            + record.source.len()
            + old.id.len();
        conn.execute("UPDATE bibliographySubscriptionSeen SET identity=?1,bytes=?2 WHERE ruleID=?3 AND source=?4 AND recordID=?5",turso::params![value,bytes as i64,id.clone(),record.source.clone(),old.id.clone()]).await?;
        add_keys(conn, id, &record.source, &old).await?;
        return Ok((false, false));
    }
    let mut matches = BTreeSet::new();
    for key in crate::discovery_merge::identity_keys(&record.article) {
        let mut rows=conn.query("SELECT DISTINCT s.groupID FROM bibliographySubscriptionKeys k JOIN bibliographySubscriptionSeen s ON s.ruleID=k.ruleID AND s.source=k.source AND s.recordID=k.recordID WHERE k.ruleID=?1 AND k.key=?2",turso::params![id.clone(),key]).await?;
        while let Some(row) = rows.next().await? {
            matches.insert(row.get::<String>(0)?);
        }
    }
    let mut eligible = Vec::new();
    let mut related = BTreeSet::new();
    for group in &matches {
        let values = members(conn, id, group).await?;
        if values
            .iter()
            .all(|a| crate::discovery_merge::compatible(a, &record.article))
        {
            eligible.push((group.clone(), values));
        } else {
            related.insert(group.clone());
        }
    }
    let bridge_safe = eligible.iter().enumerate().all(|(i, (_, a))| {
        eligible.iter().skip(i + 1).all(|(_, b)| {
            a.iter()
                .all(|x| b.iter().all(|y| crate::discovery_merge::compatible(x, y)))
        })
    });
    if bridge_safe
        && eligible
            .iter()
            .map(|(_, members)| members.len())
            .sum::<usize>()
            >= 512
    {
        return Err(SubscriptionError::Budget(
            "identity group exceeds 512 source records".into(),
        ));
    }
    let group = if bridge_safe && !eligible.is_empty() {
        // Prefer an existing inbox item, so a deleted notification is never
        // recreated merely because later metadata joins two seen groups.
        let mut selected = eligible[0].0.clone();
        for (g, _) in &eligible {
            if inbox(conn, id, g).await?.is_some() {
                selected = g.clone();
                break;
            }
        }
        selected
    } else {
        related.extend(matches);
        uuid::Uuid::new_v4().to_string()
    };
    let newly_matched = eligible.is_empty() || !bridge_safe;
    let mut item = inbox(conn, id, &group).await?;
    if bridge_safe {
        for (other, _) in &eligible {
            if other == &group {
                continue;
            }
            if let Some(other_item) = inbox(conn, id, other).await? {
                if let Some(target) = &mut item {
                    target.records.extend(other_item.records);
                    target.related_groups.extend(other_item.related_groups);
                    target.read &= other_item.read;
                }
                conn.execute(
                    "DELETE FROM bibliographySubscriptionInbox WHERE ruleID=?1 AND groupID=?2",
                    turso::params![id.clone(), other.clone()],
                )
                .await?;
                scheduling::resources_changed(conn).await?;
            }
            conn.execute(
                "UPDATE bibliographySubscriptionSeen SET groupID=?1 WHERE ruleID=?2 AND groupID=?3",
                turso::params![group.clone(), id.clone(), other.clone()],
            )
            .await?;
        }
    }
    let minimal = identity(&record.article);
    let value = serde_json::to_string(&minimal)?;
    let bytes = value.len()
        + serde_json::to_vec(&crate::discovery_merge::identity_keys(&minimal))?.len()
        + record.source.len()
        + minimal.id.len();
    conn.execute(
        "INSERT INTO bibliographySubscriptionSeen VALUES(?1,?2,?3,?4,?5,?6)",
        turso::params![
            id.clone(),
            record.source.clone(),
            record.article.id.clone(),
            group.clone(),
            value,
            bytes as i64
        ],
    )
    .await?;
    add_keys(conn, id, &record.source, &minimal).await?;
    if newly_matched {
        item = Some(SubscriptionInboxItem {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: id.clone(),
            rule_version: lease.version,
            group_id: group.clone(),
            first_matched_ms: now,
            records: Vec::new(),
            related_groups: Vec::new(),
            read: false,
        });
    }
    if let Some(mut item) = item {
        item.records.push(record);
        item.related_groups.extend(related.iter().cloned());
        item.related_groups.sort();
        item.related_groups.dedup();
        item.related_groups.retain(|g| g != &group);
        if newly_matched {
            item.read = false;
        }
        put_inbox(conn, &item, policy).await?;
    }
    for other in related {
        if let Some(mut item) = inbox(conn, id, &other).await? {
            if !item.related_groups.contains(&group) {
                item.related_groups.push(group.clone());
                put_inbox(conn, &item, policy).await?;
            }
        }
    }
    Ok((true, newly_matched))
}

impl SubscriptionStore {
    pub async fn commit_page(
        &self,
        lease: &SubscriptionLease,
        name: &str,
        sequence: u64,
        request_cursor: Option<&str>,
        page: SourcePage,
        now: i64,
    ) -> Result<SubscriptionPageReceipt> {
        let clock = OperationClock::new(now)?;
        let serialized = serde_json::to_vec(&(sequence, request_cursor, &page))?;
        let page_hash = format!("{:x}", Sha256::digest(&serialized));
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            fence(&conn, lease, clock.now()?).await?;
            let policy = limits(&conn).await?;
            if serialized.len() > policy.max_page_bytes as usize {
                return Err(SubscriptionError::Budget("source page payload".into()));
            }
            let mut state = source(&conn, &lease.rule_id, name).await?;
            if let Some(saved) = &state.last_page
                && saved.expected_sequence == sequence
                && saved.page_hash == page_hash
            {
                return Ok(saved.clone());
            }
            let plan = state
                .plan
                .as_ref()
                .ok_or_else(|| conflict("source has no prepared execution"))?;
            if state.sequence != sequence
                || state.complete
                || state.cursor.as_deref() != request_cursor
            {
                return Err(conflict("source page checkpoint changed"));
            }
            let mut ids = HashSet::new();
            if page.source != name
                || page.articles.len() > usize::from(plan.page_size)
                || plan
                    .parameters
                    .iter()
                    .any(|(key, value)| page.executed_parameters.get(key) != Some(value))
                || (page.articles.is_empty() && page.next_cursor.is_some())
                || page
                    .articles
                    .iter()
                    .any(|a| a.id.is_empty() || a.id.len() > 2048 || !ids.insert(a.id.clone()))
                || page.next_cursor.as_ref().is_some_and(|c| {
                    c.is_empty() || c.len() > 32768 || Some(c.as_str()) == request_cursor
                })
            {
                return Err(invalid(
                    "invalid source page identities, count or continuation",
                ));
            }
            let mut receipt = SubscriptionPageReceipt {
                expected_sequence: sequence,
                sequence: sequence
                    .checked_add(1)
                    .ok_or_else(|| invalid("source sequence exhausted"))?,
                page_hash,
                new_records: 0,
                new_notifications: 0,
                duplicate_records: 0,
                complete: page.next_cursor.is_none(),
            };
            if let Some(next)=&page.next_cursor {
                let hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&Some(next))?));
                let mut rows=conn.query("SELECT 1 FROM bibliographySubscriptionCursors WHERE ruleID=?1 AND source=?2 AND cursorHash=?3",turso::params![lease.rule_id.clone(),name.to_owned(),hash]).await?;
                if rows.next().await?.is_some(){return Err(invalid("source cursor loop; replay the frozen window after resolving the upstream failure"));}
            }
            let cursor_hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&request_cursor)?));
            conn.execute("INSERT INTO bibliographySubscriptionCursors VALUES(?1,?2,?3)",turso::params![lease.rule_id.clone(),name.to_owned(),cursor_hash]).await?;
            let mut rows=conn.query("SELECT COUNT(*) FROM bibliographySubscriptionCursors",()).await?;
            let count:i64=rows.next().await?.ok_or_else(||invalid("missing cursor count"))?.get(0)?;drop(rows);
            if count>i64::from(policy.max_checkpoints){return Err(SubscriptionError::Budget("retained source checkpoints".into()));}
            for (rank, article) in page.articles.iter().cloned().enumerate() {
                let record = DiscoveryRecord {
                    source: name.into(),
                    article,
                    source_rank: usize::try_from(state.scanned_records)
                        .map_err(|_| invalid("source rank exhausted"))?
                        + rank
                        + 1,
                    query_fingerprint: plan.fingerprint.clone(),
                    retrieved_at: chrono::DateTime::from_timestamp_millis(now)
                        .ok_or_else(|| invalid("invalid observation time"))?
                        .to_rfc3339(),
                    executed_parameters: page.executed_parameters.clone(),
                    query_translation: page.query_translation.clone(),
                };
                let (new, notification) = observe(&conn, lease, record, now, &policy).await?;
                receipt.new_records += usize::from(new);
                receipt.duplicate_records += usize::from(!new);
                receipt.new_notifications += usize::from(notification);
            }
            enforce_budgets(&conn, &policy).await?;
            state.sequence = receipt.sequence;
            state.cursor = page.next_cursor;
            state.complete = receipt.complete;
            state.scanned_records = state
                .scanned_records
                .checked_add(page.articles.len() as u64)
                .ok_or_else(|| invalid("source count exhausted"))?;
            state.last_checked_ms = Some(now);
            state.retry_at_ms = None;
            state.last_error = None;
            if state.complete {
                state.completed_epoch = lease.epoch;
                if let Some(window) = &state.window {
                    state.watermark_ms = Some(
                        state
                            .watermark_ms
                            .map_or(window.through_ms, |old| old.max(window.through_ms)),
                    );
                }
            }
            super::runs::committed_page(&conn,lease,&mut state,&receipt,request_cursor,page.articles.len(),clock.now()?).await?;
            state.last_page = Some(receipt.clone());
            save_source(&conn, &lease.rule_id, &state).await?;
            // A page cannot commit using its stale entry timestamp.
            fence(&conn,lease,clock.now()?).await?;
            Ok(receipt)
        }
        .await;
        finish(tx, result).await
    }
    pub async fn inbox(
        &self,
        before_seq: Option<i64>,
        limit: u16,
    ) -> Result<(Vec<SubscriptionInboxItem>, Option<i64>)> {
        if !(1..=100).contains(&limit) || before_seq.is_some_and(|n| n <= 0) {
            return Err(invalid("invalid inbox page"));
        }
        let conn = self.conn.lock().await;
        let mut rows=conn.query("SELECT seq,value FROM bibliographySubscriptionInbox WHERE seq<?1 ORDER BY seq DESC LIMIT ?2",turso::params![before_seq.unwrap_or(i64::MAX),i64::from(limit)+1]).await?;
        let mut entries = Vec::new();
        let mut last = None;
        let mut more = false;
        while let Some(row) = rows.next().await? {
            if entries.len() == usize::from(limit) {
                more = true;
                break;
            }
            last = Some(row.get(0)?);
            entries.push(serde_json::from_str(&row.get::<String>(1)?)?);
        }
        Ok((entries, if more { last } else { None }))
    }
    pub async fn delete_inbox(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            let count = conn
                .execute(
                    "DELETE FROM bibliographySubscriptionInbox WHERE id=?1",
                    [id],
                )
                .await?;
            if count == 0 {
                return Err(SubscriptionError::NotFound);
            }
            scheduling::resources_changed(&conn).await?;
            Ok(())
        }
        .await;
        finish(tx, result).await
    }
    pub async fn set_inbox_read(&self, id: &str, read: bool) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            let mut rows = conn
                .query(
                    "SELECT value FROM bibliographySubscriptionInbox WHERE id=?1",
                    [id],
                )
                .await?;
            let mut item: SubscriptionInboxItem = serde_json::from_str(
                &rows
                    .next()
                    .await?
                    .ok_or(SubscriptionError::NotFound)?
                    .get::<String>(0)?,
            )?;
            drop(rows);
            item.read = read;
            // Read state must remain editable after quotas are lowered below
            // retained payload sizes. This never adds an identity or record.
            let value = serde_json::to_string(&item)?;
            conn.execute(
                "UPDATE bibliographySubscriptionInbox SET value=?1,bytes=?2 WHERE id=?3",
                turso::params![value.clone(), value.len() as i64, id.to_owned()],
            )
            .await?;
            Ok(())
        }
        .await;
        finish(tx, result).await
    }
}
