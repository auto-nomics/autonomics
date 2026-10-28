use super::*;
use turso::transaction::{Transaction, TransactionBehavior};

pub(super) const SCHEMA:&str="CREATE TABLE IF NOT EXISTS bibliographySemanticMaintenance (
 id INTEGER PRIMARY KEY CHECK(id=1), configuration_generation TEXT NOT NULL,
 request_id TEXT NOT NULL, state TEXT NOT NULL, owner TEXT, lease_until INTEGER NOT NULL,
 value TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS bibliographySemanticStores(signature TEXT PRIMARY KEY, store_id TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS bibliographySemanticArtifacts(store_id TEXT PRIMARY KEY,lease_until INTEGER NOT NULL);";
pub(super) const LEASE_MS: i64 = 15_000;

/// Budget changes do not change vector identity. Rebind unfinished maintenance
/// atomically with its configuration, fencing the previous owner while keeping
/// its private cache. Explicit pauses remain paused.
pub(super) async fn configure(
    connection: &Connection,
    config: SemanticConfig,
) -> Result<VersionedConfig> {
    config.validate()?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate).await?;
    let result=async {
        let old=match read_configuration(connection).await {
            Ok(value)=>value,
            Err(Error::InvalidSearchQuery(_)|Error::Json(_))=>None,
            Err(error)=>return Err(error),
        };
        let generation=uuid::Uuid::new_v4().to_string();
        let maintenance=old.and_then(|old| {
            let mut equivalent=old.config;equivalent.limits=config.limits.clone();
            if !config.enabled || equivalent!=config {return None;}
            old.maintenance.filter(|job|job.state!="complete").map(|mut job| {
                job.configuration_generation=generation.clone();
                if matches!(job.state.as_str(),"queued"|"running") || (job.state=="failed" && job.last_error.as_deref().is_some_and(|e|e.starts_with("semantic resource limit:"))) {
                    job.state="queued".into();job.last_error=None;job.finished_at=None;
                }
                job
            })
        });
        connection.execute("INSERT INTO bibliographySemanticConfig VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value,generation=excluded.generation",turso::params![serde_json::to_string(&config)?,generation.clone()]).await?;
        if let Some(job)=&maintenance {
            let changed=connection.execute("UPDATE bibliographySemanticMaintenance SET configuration_generation=?1,state=?2,owner=NULL,lease_until=0,value=?3 WHERE id=1 AND request_id=?4",turso::params![generation.clone(),job.state.clone(),serde_json::to_string(job)?,job.request_id.clone()]).await?;
            if changed!=1 {return Err(Error::SearchConflict("semantic maintenance changed during budget update".into()));}
        }
        Ok(VersionedConfig {config,generation,maintenance})
    }.await;
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

pub(super) async fn request(
    connection: &Connection,
    input: SemanticMaintenanceRequest,
) -> Result<(VersionedConfig, SemanticMaintenance)> {
    if input.request_id.len() != 36
        || !uuid::Uuid::parse_str(&input.request_id)
            .is_ok_and(|id| id.to_string() == input.request_id)
        || input.expected_configuration_generation.len() > 64
    {
        return Err(invalid(
            "maintenance requires a UUID request_id and current configuration generation",
        ));
    }
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate).await?;
    let result=async {
        let mut current=read_configuration(connection).await?.ok_or_else(||invalid("enable semantic indexing before maintenance"))?;
        if let Some(job)=&current.maintenance && job.request_id==input.request_id {
            if job.action!=input.action || job.base_configuration_generation!=input.expected_configuration_generation {return Err(Error::SearchConflict("maintenance request ID was reused for a different action".into()));}
            return Ok((current.clone(),job.clone()));
        }
        if current.generation!=input.expected_configuration_generation {return Err(Error::SearchConflict("semantic configuration changed; refresh before maintenance".into()));}
        if !current.config.enabled {return Err(invalid("enable semantic indexing before maintenance"));}
        if input.action==SemanticMaintenanceAction::Repair {
            let mut rows=connection.query("SELECT 1 FROM bibliographySemanticArtifacts WHERE store_id=?1",turso::params![input.request_id.clone()]).await?;
            if rows.next().await?.is_some(){return Err(Error::SearchConflict("repair request ID is already associated with an index store".into()));}
        }
        let generation=uuid::Uuid::new_v4().to_string();
        let store_generation=if input.action==SemanticMaintenanceAction::Repair {Some(input.request_id.clone())}
            else if matches!(input.action,SemanticMaintenanceAction::Resume|SemanticMaintenanceAction::Cancel) {current.maintenance.as_ref().filter(|job|job.state!="complete").and_then(|job|job.store_generation.clone())}else{None};
        let job=SemanticMaintenance {request_id:input.request_id,action:input.action,
            base_configuration_generation:current.generation.clone(),configuration_generation:generation.clone(),
            state:if input.action==SemanticMaintenanceAction::Cancel {"cancelled"}else{"queued"}.into(),
            requested_at:chrono::Utc::now().to_rfc3339(),finished_at:None,progress:None,last_error:None,store_generation};
        connection.execute("UPDATE bibliographySemanticConfig SET generation=?1 WHERE id=1",turso::params![generation.clone()]).await?;
        connection.execute("INSERT INTO bibliographySemanticMaintenance VALUES(1,?1,?2,?3,NULL,0,?4) ON CONFLICT(id) DO UPDATE SET configuration_generation=excluded.configuration_generation,request_id=excluded.request_id,state=excluded.state,owner=NULL,lease_until=0,value=excluded.value",
            turso::params![generation.clone(),job.request_id.clone(),job.state.clone(),serde_json::to_string(&job)?]).await?;
        current.generation=generation;current.maintenance=Some(job.clone());
        Ok((current,job))
    }.await;
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

/// Only one process may compute an explicit maintenance job. Heartbeats also
/// guard progress/completion, so an expired owner cannot replace a new owner.
pub(super) async fn claim(
    connection: &Connection,
    job: &mut SemanticMaintenance,
    owner: &str,
) -> Result<bool> {
    job.state = "running".into();
    job.last_error = None;
    job.finished_at = None;
    let now = chrono::Utc::now().timestamp_millis();
    Ok(connection.execute("UPDATE bibliographySemanticMaintenance SET state='running',owner=?1,lease_until=?2,value=?3 WHERE id=1 AND configuration_generation=?4 AND request_id=?5 AND (state='queued' OR (state='running' AND lease_until<?6))",
        turso::params![owner,now+LEASE_MS,serde_json::to_string(job)?,job.configuration_generation.clone(),job.request_id.clone(),now]).await?==1)
}
pub(super) async fn heartbeat(
    connection: &Connection,
    job: &SemanticMaintenance,
    owner: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let count=connection.execute("UPDATE bibliographySemanticMaintenance SET lease_until=?1,value=?2 WHERE id=1 AND configuration_generation=?3 AND request_id=?4 AND state='running' AND owner=?5 AND lease_until>=?6",
        turso::params![now+LEASE_MS,serde_json::to_string(job)?,job.configuration_generation.clone(),job.request_id.clone(),owner,now]).await?;
    if count != 1 {
        return Err(Error::SearchConflict(
            "semantic maintenance was cancelled or claimed by another worker".into(),
        ));
    }
    connection
        .execute(
            "UPDATE bibliographySemanticArtifacts SET lease_until=?1 WHERE store_id=?2",
            turso::params![
                now + LEASE_MS,
                job.store_generation.as_deref().unwrap_or("")
            ],
        )
        .await?;
    Ok(())
}
pub(super) async fn finish(
    connection: &Connection,
    job: &SemanticMaintenance,
    owner: &str,
    activation: Option<(&str, &str)>,
) -> Result<()> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate).await?;
    let result=async {
        let count=connection.execute("UPDATE bibliographySemanticMaintenance SET state=?1,lease_until=0,value=?2 WHERE id=1 AND configuration_generation=?3 AND request_id=?4 AND state='running' AND owner=?5 AND lease_until>=?6 AND EXISTS(SELECT 1 FROM bibliographySemanticConfig WHERE id=1 AND generation=?3)",
            turso::params![job.state.clone(),serde_json::to_string(job)?,job.configuration_generation.clone(),job.request_id.clone(),owner,chrono::Utc::now().timestamp_millis()]).await?;
        if count!=1 {return Err(Error::SearchConflict("semantic maintenance was superseded before completion".into()));}
        if let Some((signature,store))=activation {
            connection.execute("INSERT INTO bibliographySemanticStores VALUES(?1,?2) ON CONFLICT(signature) DO UPDATE SET store_id=excluded.store_id",turso::params![signature,store]).await?;
        }
        Ok(())
    }.await;
    match result {
        Ok(()) => tx.commit().await.map_err(Into::into),
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

/// Retain active stores and live repair attempts. Unreferenced private stores
/// are removed only after their lease plus a five-minute grace period. The
/// original shared base store is never unlinked by this maintenance GC.
pub(super) async fn cleanup(connection: &Connection, base: &str) -> Result<()> {
    if base == ":memory:" {
        return Ok(());
    }
    let cutoff = chrono::Utc::now().timestamp_millis() - 300_000;
    let mut rows=connection.query("SELECT store_id FROM bibliographySemanticArtifacts WHERE lease_until<?1 AND store_id NOT IN(SELECT store_id FROM bibliographySemanticStores) AND store_id NOT IN(SELECT json_extract(value,'$.store_generation') FROM bibliographySemanticMaintenance WHERE configuration_generation=(SELECT generation FROM bibliographySemanticConfig WHERE id=1) AND json_extract(value,'$.store_generation') IS NOT NULL) LIMIT 16",turso::params![cutoff]).await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<String>(0)?);
    }
    drop(rows);
    for id in ids {
        if uuid::Uuid::parse_str(&id).is_err() {
            return Err(invalid("invalid semantic staging store identity"));
        }
        let path = store_path(base, &id);
        let removed = tokio::task::spawn_blocking(move || {
            for suffix in ["", "-wal", "-tshm"] {
                if let Err(error) = std::fs::remove_file(format!("{path}{suffix}"))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return false;
                }
            }
            true
        })
        .await
        .map_err(|_| Error::Unknown("semantic artifact cleanup task failed".into()))?;
        if removed {
            connection.execute("DELETE FROM bibliographySemanticArtifacts WHERE store_id=?1 AND lease_until<?2 AND store_id NOT IN(SELECT store_id FROM bibliographySemanticStores)",turso::params![id,cutoff]).await?;
        }
    }
    Ok(())
}
