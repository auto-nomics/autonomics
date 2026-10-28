//! Bounded sidecar maintenance. Head publication never cascades a library-sized
//! binding deletion or materializes all live input hashes in one transaction.
use super::*;
const BINDING_PAGE: usize = 2048;
const GENERATION_PAGES: usize = 4;
const VECTOR_PAGE: usize = 256;
const VECTOR_PAGES: usize = 16;

pub(super) async fn retire(store: &Connection, generation: &str) -> Result<()> {
    store.execute("UPDATE semanticGenerations SET touchedAt=0 WHERE generation=?1 AND NOT EXISTS(SELECT 1 FROM semanticHeads WHERE generation=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindingAliases WHERE bindingGeneration=?1)",turso::params![generation.to_owned()]).await?;
    // Empty aliases/staging rows are cheap. Nonempty generations are left to
    // the bounded collector; FK cascading can never reach their binding set.
    store.execute("DELETE FROM semanticGenerations WHERE generation=?1 AND NOT EXISTS(SELECT 1 FROM semanticHeads WHERE generation=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindingAliases WHERE bindingGeneration=?1) AND NOT EXISTS(SELECT 1 FROM semanticBindings WHERE generation=?1)",turso::params![generation.to_owned()]).await?;
    Ok(())
}
pub(super) async fn reap(store: &Connection) -> Result<()> {
    for _ in 0..GENERATION_PAGES {
        let tx = Transaction::new_unchecked(store, TransactionBehavior::Immediate).await?;
        let result=async {
            let mut rows=store.query("SELECT generation FROM semanticGenerations g WHERE touchedAt<?1 AND NOT EXISTS(SELECT 1 FROM semanticHeads h WHERE h.generation=g.generation) AND NOT EXISTS(SELECT 1 FROM semanticBindingAliases a WHERE a.bindingGeneration=g.generation) ORDER BY touchedAt,generation LIMIT 1",turso::params![chrono::Utc::now().timestamp_millis()-300_000]).await?;
            let generation=rows.next().await?.map(|r|r.get::<String>(0)).transpose()?;drop(rows);
            let Some(generation)=generation else{return Ok(false);};
            let mut rows=store.query("SELECT chunkID FROM semanticBindings WHERE generation=?1 ORDER BY chunkID LIMIT ?2",turso::params![generation.clone(),BINDING_PAGE as i64]).await?;
            let mut last=None;
            while let Some(row)=rows.next().await?{last=Some(row.get::<String>(0)?);}
            drop(rows);
            if let Some(last)=last {
                store.execute("DELETE FROM semanticBindings INDEXED BY sqlite_autoindex_semanticBindings_1 WHERE generation=?1 AND chunkID<=?2",turso::params![generation.clone(),last]).await?;
            }
            retire(store,&generation).await?;
            Ok(true)
        }.await;
        match result {
            Ok(more) => {
                tx.commit().await?;
                if !more {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        }
    }
    Ok(())
}
async fn vectors(store: &Connection, signature: &str) -> Result<()> {
    for _ in 0..VECTOR_PAGES {
        let tx = Transaction::new_unchecked(store, TransactionBehavior::Immediate).await?;
        let result=async {
            store.execute("INSERT OR IGNORE INTO semanticGarbageCollection VALUES(?1,'')",[signature]).await?;
            let mut rows=store.query("SELECT afterInputHash FROM semanticGarbageCollection WHERE signature=?1",[signature]).await?;
            let after:String=rows.next().await?.ok_or_else(||invalid("missing vector cleanup checkpoint"))?.get(0)?;drop(rows);
            let mut rows=store.query("SELECT v.inputHash,EXISTS(SELECT 1 FROM semanticBindings b INDEXED BY semanticBindings_input JOIN semanticGenerations g ON g.generation=b.generation WHERE b.inputHash=v.inputHash AND g.signature=v.signature) FROM semanticVectors v WHERE v.signature=?1 AND v.inputHash>?2 ORDER BY v.inputHash LIMIT ?3",turso::params![signature.to_owned(),after,VECTOR_PAGE as i64]).await?;
            let mut last=None;let mut unused=Vec::new();
            while let Some(row)=rows.next().await? {
                let input:String=row.get(0)?;
                if row.get::<i64>(1)?==0{unused.push(input.clone());}
                last=Some(input);
            }
            drop(rows);
            // All reference checks and deletion are under this writer lease.
            for input in unused {
                store.execute("DELETE FROM semanticVectors WHERE signature=?1 AND inputHash=?2",turso::params![signature.to_owned(),input]).await?;
            }
            let more=last.is_some();
            store.execute("UPDATE semanticGarbageCollection SET afterInputHash=?1 WHERE signature=?2",turso::params![last.unwrap_or_default(),signature.to_owned()]).await?;
            Ok(more)
        }.await;
        match result {
            Ok(more) => {
                tx.commit().await?;
                if !more {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        }
    }
    Ok(())
}
impl SemanticIndex {
    /// Idle/resource-wait work does not contact a model or alter the head.
    /// Failed batches are retained for retry while the source is ahead of the
    /// published head, and in stores that have never published a head.
    pub(crate) async fn cleanup_bounded(&mut self) {
        let result = async {
            reap(&self.store).await?;
            if let Some(head) = self.head().await?
                && source_version(&self.source).await? == (head.revision, head.token)
            {
                vectors(&self.store, &self.signature).await?;
            }
            Ok::<_, Error>(())
        }
        .await;
        self.cleanup_warning = result
            .err()
            .map(|error| format!("semantic cleanup pending: {error}"));
    }
    pub(crate) fn cleanup_warning(&self) -> Option<String> {
        self.cleanup_warning.clone()
    }
}
