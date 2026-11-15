//! Atomic interchange import and explicitly confirmed record merge.
use crate::bib_base::{BibCore, sync_search_index};
use crate::{BibBase, Error, Result};
use bib_types::{AddedBy, Article, ArticleRole};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use turso::transaction::{Transaction, TransactionBehavior};
#[derive(Debug, Clone, Serialize)]
pub struct LibraryImportOutcome {
    pub imported_ids: Vec<String>,
    pub skipped_ids: Vec<String>,
}
fn identity_key(identifier: &bib_types::Identifier) -> String {
    if matches!(identifier.kind, bib_types::IdKind::Other) {
        return format!("other:{}", identifier.value.trim());
    }
    crate::discovery_merge::identifier_key(identifier).unwrap_or_else(|| {
        format!(
            "{}:{}",
            identifier.kind.as_str(),
            identifier.value.trim().to_lowercase()
        )
    })
}
pub fn library_identity_keys(article: &Article) -> Vec<String> {
    article.identifiers.iter().map(identity_key).collect()
}
async fn asset_revision(conn: &turso::Connection, id: &str) -> Result<String> {
    let mut rows=conn.query("SELECT i.key,COALESCE(a.vfsPath,''),COALESCE(a.storageHash,''),COALESCE(f.extractionStatus,''),COALESCE(f.extractedTextFormat,''),COALESCE(f.extractedBy,''),COALESCE(f.vfsExtractedPath,''),COALESCE(f.textContent,'') FROM articleItems m JOIN itemAttachments a ON a.parentItemID=m.itemID JOIN items i ON i.itemID=a.itemID LEFT JOIN fulltextItems f ON f.itemID=a.itemID WHERE m.article_id=?1 ORDER BY i.key",turso::params![id]).await?;
    let mut hash = Sha256::new();
    while let Some(row) = rows.next().await? {
        for index in 0..8 {
            let value = row.get::<String>(index)?;
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl BibCore {
    pub(crate) async fn library_merge_asset_revisions(
        &self,
        ids: Vec<String>,
    ) -> Result<HashMap<String, String>> {
        let conn = self.conn();
        let mut result = HashMap::new();
        for id in ids {
            let revision = asset_revision(&conn, &id).await?;
            result.insert(id, revision);
        }
        Ok(result)
    }

    pub(crate) async fn import_library_records(
        &self,
        articles: Vec<Article>,
        collection: Option<String>,
    ) -> Result<LibraryImportOutcome> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            let mut outcome = LibraryImportOutcome {
                imported_ids: Vec::new(),
                skipped_ids: Vec::new(),
            };
            let mut identity_map = HashMap::<String, HashSet<String>>::new();
            let mut rows = tx
                .query("SELECT article_id,kind,value FROM identifiers", ())
                .await?;
            while let Some(row) = rows.next().await? {
                let id = row.get::<String>(0)?;
                let kind = row.get::<String>(1)?;
                let value = row.get::<String>(2)?;
                if let Ok(kind) =
                    serde_json::from_value::<bib_types::IdKind>(serde_json::json!(kind))
                {
                    let identifier = bib_types::Identifier::new(kind, value);
                    let key = identity_key(&identifier);
                    identity_map.entry(key).or_default().insert(id);
                }
            }
            drop(rows);
            for article in articles {
                let mut matches = HashSet::new();
                if let Some(existing)=Self::get_article_on(&tx,&article.id).await? { matches.insert(existing.id); }
                for key in library_identity_keys(&article) { if let Some(ids)=identity_map.get(&key) { matches.extend(ids.iter().cloned()); } }
                if matches.len()>1 { return Err(Error::Unknown(format!("Ambiguous identifiers for {} match multiple records; resolve conflicts before importing",article.title))); }
                let existing=matches.into_iter().next();
                if let Some(id) = existing {
                    outcome.skipped_ids.push(id);
                    continue;
                }
                Self::upsert_article_in_tx(&tx, &article).await?;
                crate::zotero_model::sync_article_item(&tx, &article.id).await?;
                if let Some(collection) = &collection {
                    Self::add_to_collection_in_tx(
                        &tx,
                        collection,
                        &article.id,
                        ArticleRole::Referenced,
                        AddedBy::User,
                        None,
                    )
                    .await?;
                }
                sync_search_index(&tx, &article.id).await?;
                for key in library_identity_keys(&article) {
                    identity_map.entry(key).or_default().insert(article.id.clone());
                }
                outcome.imported_ids.push(article.id);
            }
            Ok(outcome)
        }
        .await;
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
    pub(crate) async fn merge_library_articles(
        &self,
        target_id: String,
        source_ids: Vec<String>,
        expected: HashMap<String, String>,
        metadata: Option<Article>,
        expected_assets: Option<HashMap<String, String>>,
    ) -> Result<Article> {
        let unique: HashSet<_> = source_ids.iter().collect();
        if unique.len() != source_ids.len()
            || source_ids.is_empty()
            || source_ids.len() > 100
            || source_ids.contains(&target_id)
        {
            return Err(Error::Unknown(
                "Select 1–100 distinct source records and a separate target".into(),
            ));
        }
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result=async{
   let mut records=Vec::new();for id in std::iter::once(&target_id).chain(source_ids.iter()){
    if crate::reliability::is_trashed(&tx,id).await?{return Err(Error::NotFound(id.clone()));}
    let article=Self::get_article_on(&tx,id).await?.ok_or_else(||Error::NotFound(id.clone()))?;
    if expected.get(id)!=Some(&crate::article_revision(&article)){return Err(Error::SearchConflict("Records changed; reload duplicate review before merging".into()));}records.push(article);
   }
   let mut target=records.remove(0);
   if let Some(metadata)=metadata { target.title=metadata.title;target.authors=metadata.authors;target.year=metadata.year;target.journal=metadata.journal;target.volume=metadata.volume;target.issue=metadata.issue;target.pages=metadata.pages;target.abstract_text=metadata.abstract_text;target.extra_fields=metadata.extra_fields; }
   target.title=target.title.trim().into();for author in &mut target.authors { author.last_name=author.last_name.trim().into(); }
   if target.year.is_some_and(|year| year==0 || year>9999) || target.authors.iter().any(|author|author.last_name.trim().is_empty()) { return Err(Error::Unknown("Invalid year or author in merged metadata".into())); }
   if target.title.trim().is_empty(){return Err(Error::Unknown("The merged title cannot be empty".into()));}
   let target_item=crate::zotero_model::ensure_article_item(&tx,&target_id).await?;
   let mut rows=tx.query("SELECT libraryID FROM items WHERE itemID=?1",turso::params![target_item]).await?;let library=rows.next().await?.ok_or_else(||Error::NotFound(target_id.clone()))?.get::<i64>(0)?;drop(rows);
   for source in records{
    if let Some(expected_assets)=&expected_assets { if expected_assets.get(&source.id)!=Some(&asset_revision(&tx,&source.id).await?) { return Err(Error::SearchConflict("Attachments changed while preparing merge; review again".into())); } }
    let source_item=crate::zotero_model::ensure_article_item(&tx,&source.id).await?;
    let mut rows=tx.query("SELECT libraryID FROM items WHERE itemID=?1",turso::params![source_item]).await?;let source_library=rows.next().await?.ok_or_else(||Error::NotFound(source.id.clone()))?.get::<i64>(0)?;drop(rows);
    if library!=source_library{return Err(Error::Unknown("Records in different libraries cannot be merged".into()));}
    for identifier in source.identifiers {if let Some(current)=target.identifiers.iter().find(|i|i.kind==identifier.kind){if current.value!=identifier.value{return Err(Error::Unknown(format!("Conflicting {} identifiers; edit records before merging",identifier.kind.as_str())));}}else{target.identifiers.push(identifier);}}
    for (key,value) in source.extra_fields { target.extra_fields.entry(key).or_insert(value); }
    for keyword in source.keywords{if !target.keywords.contains(&keyword){target.keywords.push(keyword);}}
    let mut active = tx.query("SELECT 1 FROM fulltextItems f JOIN itemAttachments a ON a.itemID=f.itemID WHERE a.parentItemID=?1 AND (f.extractionStatus='running' OR (f.extractionStatus='pending' AND a.vfsPath IS NOT NULL AND a.linkMode<>3)) LIMIT 1",turso::params![source_item]).await?;
    if active.next().await?.is_some() { return Err(Error::Unknown("Wait for attachment extraction to finish before merging".into())); }
    drop(active);
    // Move ownership; stored object URIs and attachment keys stay unchanged.
    tx.execute("UPDATE itemAttachments SET parentItemID=?1 WHERE parentItemID=?2",turso::params![target_item,source_item]).await?;
    tx.execute("UPDATE itemNotes SET parentItemID=?1 WHERE parentItemID=?2",turso::params![target_item,source_item]).await?;
    tx.execute("UPDATE annotations SET article_id=?1 WHERE article_id=?2",turso::params![target_id.clone(),source.id.clone()]).await?;
    tx.execute("INSERT OR IGNORE INTO collectionItems(collectionID,itemID,orderIndex,role,fetch_status,added_by,note,added_at) SELECT collectionID,?1,orderIndex,role,fetch_status,added_by,note,added_at FROM collectionItems WHERE itemID=?2",turso::params![target_item,source_item]).await?;
    // Transfer a preferred file only when the target has none. Clear the unique source pointer first.
    let mut rows=tx.query("SELECT attachmentItemID FROM articleItems WHERE article_id=?1",turso::params![source.id.clone()]).await?;let preferred=rows.next().await?.and_then(|row|row.get::<i64>(0).ok());drop(rows);
    tx.execute("UPDATE articleItems SET attachmentItemID=NULL WHERE article_id=?1",turso::params![source.id.clone()]).await?;
    if let Some(preferred)=preferred{tx.execute("UPDATE articleItems SET attachmentItemID=?2 WHERE article_id=?1 AND attachmentItemID IS NULL",turso::params![target_id.clone(),preferred]).await?;}
    tx.execute("UPDATE fulltexts SET article_id=?1 WHERE article_id=?2 AND NOT EXISTS(SELECT 1 FROM fulltexts WHERE article_id=?1)",turso::params![target_id.clone(),source.id.clone()]).await?;
    crate::zotero_model::delete_article_item(&tx,&source.id).await?;
    tx.execute("DELETE FROM articles WHERE id=?1",turso::params![source.id.clone()]).await?;
    sync_search_index(&tx,&source.id).await?;
   }
   crate::reliability::ensure_identifiers_available(&tx,&target).await?;
   target.updated_at=Some(chrono::Utc::now());Self::upsert_article_in_tx(&tx,&target).await?;crate::zotero_model::sync_article_item(&tx,&target_id).await?;sync_search_index(&tx,&target_id).await?;
   Self::get_article_on(&tx,&target_id).await?.ok_or_else(||Error::NotFound(target_id))
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
}
impl BibBase {
    pub async fn library_merge_asset_revisions(
        &self,
        ids: Vec<String>,
    ) -> Result<HashMap<String, String>> {
        self.call(|reply| crate::actor::BibCommand::LibraryMergeAssetRevisions { ids, reply })
            .await
    }

    pub async fn import_library_records(
        &self,
        articles: Vec<Article>,
        collection: Option<String>,
    ) -> Result<LibraryImportOutcome> {
        self.call(|reply| crate::actor::BibCommand::ImportLibraryRecords {
            articles,
            collection,
            reply,
        })
        .await
    }
    pub async fn merge_library_articles(
        &self,
        target_id: String,
        source_ids: Vec<String>,
        expected: HashMap<String, String>,
    ) -> Result<Article> {
        self.merge_library_articles_with_metadata(target_id, source_ids, expected, None)
            .await
    }
    pub async fn merge_library_articles_with_metadata(
        &self,
        target_id: String,
        source_ids: Vec<String>,
        expected: HashMap<String, String>,
        metadata: Option<Article>,
    ) -> Result<Article> {
        self.merge_library_articles_checked(target_id, source_ids, expected, metadata, None)
            .await
    }
    pub async fn merge_library_articles_checked(
        &self,
        target_id: String,
        source_ids: Vec<String>,
        expected: HashMap<String, String>,
        metadata: Option<Article>,
        expected_assets: Option<HashMap<String, String>>,
    ) -> Result<Article> {
        self.call(|reply| crate::actor::BibCommand::MergeLibraryArticles {
            target_id,
            source_ids,
            expected,
            metadata,
            expected_assets,
            reply,
        })
        .await
    }
}
