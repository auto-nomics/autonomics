//! Reviewed discovery imports. Matching and each item's write share an
//! immediate transaction; one failed item never rolls back successful peers.
use crate::discovery_merge::{compatible, identifier_key, identity_keys, preprint, published};
use crate::{BibBase, Error, Result, actor::BibCommand, bib_base::BibCore};
use bib_types::{AddedBy, Article, ArticleRole, IdKind, Identifier, discovery::*};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};

pub(crate) async fn ensure(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS discoveryImportProvenance (
        importKey TEXT PRIMARY KEY, articleID TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
        selectedSource TEXT NOT NULL, sourceRecordID TEXT NOT NULL, payload TEXT NOT NULL, importedAt TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS discoveryImportOrigin ON discoveryImportProvenance(selectedSource,sourceRecordID);
        CREATE TABLE IF NOT EXISTS discoveryImportIdentities (
        articleID TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE, identityKey TEXT NOT NULL,
        PRIMARY KEY(articleID,identityKey));
        CREATE INDEX IF NOT EXISTS discoveryIdentityLookup ON discoveryImportIdentities(identityKey);
        CREATE TABLE IF NOT EXISTS discoveryItemLinks (
        articleID TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
        relatedArticleID TEXT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
        relation TEXT NOT NULL, PRIMARY KEY(articleID,relatedArticleID));").await?;
    conn.execute(
        "INSERT OR IGNORE INTO bib_meta(key,value) VALUES ('discovery_identity_revision','0')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR IGNORE INTO bib_meta(key,value) VALUES ('discovery_identity_token',?1)",
        turso::params![uuid::Uuid::new_v4().to_string()],
    )
    .await?;
    Ok(())
}
type IdentityIndex = HashMap<String, BTreeSet<String>>;
struct Cache {
    marker: Vec<String>,
    index: IdentityIndex,
}
async fn marker(conn: &Connection) -> Result<Vec<String>> {
    let mut rows=conn.query("SELECT key,value FROM bib_meta WHERE key IN ('search_revision','search_revision_token','discovery_identity_revision','discovery_identity_token') ORDER BY key",()).await?;
    let mut marker = Vec::new();
    while let Some(row) = rows.next().await? {
        marker.push(row.get::<String>(1)?);
    }
    Ok(marker)
}
fn kind(value: &str) -> Option<IdKind> {
    Some(match value {
        "doi" => IdKind::Doi,
        "biorxiv" => IdKind::Biorxiv,
        "pmid" => IdKind::Pmid,
        "pmc" => IdKind::Pmc,
        "arxiv" => IdKind::Arxiv,
        "openalex" => IdKind::OpenAlex,
        "s2" => IdKind::S2,
        "embase" => IdKind::Embase,
        _ => return None,
    })
}
fn index_insert(index: &mut IdentityIndex, key: String, id: String) {
    let bucket = index.entry(key).or_default();
    if bucket.len() < 51 {
        bucket.insert(id);
    }
}
async fn load_index(
    conn: &Connection,
    library: i64,
    wanted: &HashSet<String>,
) -> Result<IdentityIndex> {
    let mut index = IdentityIndex::new();
    let mut rows=conn.query("SELECT d.article_id,d.kind,d.value FROM identifiers d JOIN articleItems ai ON ai.article_id=d.article_id JOIN items i USING(itemID) WHERE i.libraryID=?1",turso::params![library]).await?;
    while let Some(row) = rows.next().await? {
        if let Some(kind) = kind(&row.get::<String>(1)?) {
            if let Some(key) = identifier_key(&Identifier::new(kind, row.get::<String>(2)?)) {
                if wanted.contains(&key) {
                    index_insert(&mut index, key, row.get(0)?);
                }
            }
        }
    }
    drop(rows);
    let mut rows=conn.query("SELECT d.articleID,d.identityKey FROM discoveryImportIdentities d JOIN articleItems ai ON ai.article_id=d.articleID JOIN items i USING(itemID) WHERE i.libraryID=?1",turso::params![library]).await?;
    while let Some(row) = rows.next().await? {
        let key = row.get::<String>(1)?;
        if wanted.contains(&key) {
            index_insert(&mut index, key, row.get(0)?);
        }
    }
    Ok(index)
}
async fn target(
    conn: &Connection,
    library: Option<i64>,
    collection: Option<&str>,
) -> Result<(i64, bool)> {
    if library.is_some_and(|id| id <= 0) {
        return Err(Error::InvalidCollection(
            "library_id must be positive".into(),
        ));
    }
    let target = if let Some(collection) = collection {
        let col = crate::collections::stored_collection(conn, collection).await?;
        if col.deleted {
            return Err(Error::CollectionConflict(
                "target collection is in trash".into(),
            ));
        }
        if library.is_some_and(|id| id != col.library_id) {
            return Err(Error::InvalidCollection(
                "collection belongs to another library".into(),
            ));
        }
        col.library_id
    } else {
        library.unwrap_or(1)
    };
    let mut rows = conn
        .query(
            "SELECT editable,archived FROM libraries WHERE libraryID=?1",
            turso::params![target],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| Error::NotFound(format!("library {target}")))?;
    Ok((target, row.get::<i64>(0)? != 0 && row.get::<i64>(1)? == 0))
}
fn validate_record(record: &DiscoveryImportRecord) -> Result<String> {
    if record.selected.source.trim().is_empty()
        || record.selected.article.id.trim().is_empty()
        || record.selected.source_rank == 0
    {
        return Err(Error::InvalidSearchQuery(
            "source record is missing a stable identity or rank".into(),
        ));
    }
    if record.schema_version != 1 {
        return Err(Error::InvalidSearchQuery(
            "unsupported discovery provenance version".into(),
        ));
    }
    if record.selected.article.title.trim().is_empty() {
        return Err(Error::InvalidSearchQuery(
            "source record has no title".into(),
        ));
    }
    if format!("{:x}", Sha256::digest(serde_json::to_vec(&record.records)?)) != record.fingerprint {
        return Err(Error::SearchConflict(
            "source provenance fingerprint does not match its records".into(),
        ));
    }
    let selected = serde_json::to_value(&record.selected)?;
    if !record
        .records
        .iter()
        .any(|source| serde_json::to_value(source).is_ok_and(|value| value == selected))
    {
        return Err(Error::InvalidSearchQuery(
            "selected source is absent from retained provenance".into(),
        ));
    }
    let mut kinds = HashSet::new();
    if record
        .selected
        .article
        .identifiers
        .iter()
        .any(|id| id.value.trim().is_empty() || !kinds.insert(id.kind.as_str()))
    {
        return Err(Error::InvalidSearchQuery(
            "source record has empty or conflicting identifiers; choose another source variant"
                .into(),
        ));
    }
    let payload = serde_json::to_string(record)?;
    if payload.len() > 2 * 1024 * 1024 {
        return Err(Error::InvalidSearchQuery(
            "source provenance exceeds the 2 MiB per-item import limit".into(),
        ));
    }
    Ok(payload)
}
fn canonical_id(record: &DiscoveryImportRecord, library: i64) -> String {
    let mut keys = identity_keys(&record.selected.article);
    keys.sort_by_key(|key| {
        (
            if key.starts_with("doi:") {
                0
            } else if key.starts_with("pmid:") {
                1
            } else {
                2
            },
            key.clone(),
        )
    });
    let basis = if let Some(key) = keys.first() {
        let edition = if key.starts_with("arxiv:") {
            if preprint(&record.selected.article) {
                "preprint"
            } else if published(&record.selected.article) {
                "published"
            } else {
                "unknown"
            }
        } else {
            ""
        };
        serde_json::json!([key, edition])
    } else {
        serde_json::json!([record.selected.source, record.selected.article.id])
    };
    format!(
        "discovery-{library}-{:x}",
        Sha256::digest(basis.to_string())
    )
}
async fn matches(
    conn: &Connection,
    index: &IdentityIndex,
    record: &DiscoveryImportRecord,
    library: i64,
) -> Result<(Vec<Article>, Vec<Article>)> {
    let mut ids = BTreeSet::new();
    'keys: for key in identity_keys(&record.selected.article) {
        if let Some(found) = index.get(&key) {
            for id in found {
                ids.insert(id.clone());
                if ids.len() > 50 {
                    break 'keys;
                }
            }
        }
    }
    let mut rows=conn.query("SELECT DISTINCT d.articleID FROM discoveryImportProvenance d JOIN articleItems ai ON ai.article_id=d.articleID JOIN items i USING(itemID) WHERE d.selectedSource=?1 AND d.sourceRecordID=?2 AND i.libraryID=?3 LIMIT 51",turso::params![record.selected.source.clone(),record.selected.article.id.clone(),library]).await?;
    while let Some(row) = rows.next().await? {
        ids.insert(row.get::<String>(0)?);
    }
    drop(rows);
    if ids.len() > 50 {
        return Err(Error::SearchConflict(
            "too many existing records share these identifiers; review library duplicates".into(),
        ));
    }
    let articles =
        BibCore::hydrate_article_ids_on(conn, &ids.into_iter().collect::<Vec<_>>()).await?;
    let mut existing = Vec::new();
    let mut related = Vec::new();
    for article in articles {
        let mut effective = article.clone();
        let mut rows=conn.query("SELECT identityKey FROM discoveryImportIdentities WHERE articleID=?1 AND identityKey LIKE 'doi:%'",turso::params![article.id.clone()]).await?;
        let primary_doi = effective.identifiers.iter().any(|id| {
            matches!(id.kind, IdKind::Doi | IdKind::Biorxiv) && !id.value.trim().is_empty()
        });
        while let Some(row) = rows.next().await? {
            if !primary_doi {
                effective.identifiers.push(Identifier::doi(
                    row.get::<String>(0)?.trim_start_matches("doi:"),
                ));
            }
        }
        if compatible(&effective, &record.selected.article) {
            existing.push(article);
        } else {
            related.push(article);
        }
    }
    Ok((existing, related))
}
fn differences(record: &DiscoveryImportRecord, existing: &Article) -> Vec<FieldConflict> {
    let source = serde_json::to_value(&record.selected.article).expect("article serializes");
    let local = serde_json::to_value(existing).expect("article serializes");
    [
        "title",
        "authors",
        "identifiers",
        "abstract_text",
        "year",
        "month",
        "journal",
        "volume",
        "issue",
        "pages",
        "language",
        "pub_types",
        "keywords",
    ]
    .into_iter()
    .filter_map(|field| {
        if source[field] == local[field] {
            None
        } else {
            Some(FieldConflict {
                field: field.into(),
                variants: vec![
                    FieldVariant {
                        source: "local_library".into(),
                        source_record_id: existing.id.clone(),
                        value: local[field].clone(),
                    },
                    FieldVariant {
                        source: record.selected.source.clone(),
                        source_record_id: record.selected.article.id.clone(),
                        value: source[field].clone(),
                    },
                ],
            })
        }
    })
    .collect()
}

impl BibBase {
    pub async fn discovery_provenance(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<DiscoveryProvenancePage> {
        self.call(|reply| BibCommand::DiscoveryProvenance {
            id: id.into(),
            offset,
            limit,
            reply,
        })
        .await
    }
    pub async fn preview_discovery_import(
        &self,
        records: Vec<DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
    ) -> Result<DiscoveryImportPreview> {
        self.call(|reply| BibCommand::PreviewDiscoveryImport {
            records,
            library,
            collection,
            reply,
        })
        .await
    }
    pub async fn import_discovery(
        &self,
        records: Vec<DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
    ) -> Result<Vec<DiscoveryImportOutcome>> {
        let outcomes = self
            .call(|reply| BibCommand::ImportDiscovery {
                records,
                library,
                collection,
                reply,
            })
            .await?;
        if outcomes.iter().any(|item| item.changed) {
            self.changes.send_modify(|version| *version += 1);
        }
        Ok(outcomes)
    }
}
impl BibCore {
    pub(crate) async fn discovery_provenance(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<DiscoveryProvenancePage> {
        if !(1..=100).contains(&limit) || offset > i64::MAX as usize {
            return Err(Error::InvalidSearchQuery(
                "invalid provenance pagination".into(),
            ));
        }
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result=async {
            let mut rows=tx.query("SELECT 1 FROM articles WHERE id=?1",turso::params![id]).await?;if rows.next().await?.is_none(){return Err(Error::NotFound(format!("article {id}")));}drop(rows);
            let mut rows=tx.query("SELECT COUNT(*) FROM discoveryImportProvenance WHERE articleID=?1",turso::params![id]).await?;let total=rows.next().await?.ok_or_else(||Error::Unknown("missing provenance count".into()))?.get::<i64>(0)? as usize;drop(rows);
            let mut rows=tx.query("SELECT importedAt,payload FROM discoveryImportProvenance WHERE articleID=?1 ORDER BY importedAt,importKey LIMIT ?2 OFFSET ?3",turso::params![id,limit as i64,offset as i64]).await?;let mut entries=Vec::new();
            while let Some(row)=rows.next().await? {let provenance:DiscoveryImportRecord=serde_json::from_str(&row.get::<String>(1)?)?;if provenance.schema_version!=1{return Err(Error::Unknown("unsupported stored discovery provenance version".into()));}entries.push(DiscoveryStoredProvenance{imported_at:row.get(0)?,provenance});}drop(rows);
            let mut rows=tx.query("SELECT relatedArticleID FROM discoveryItemLinks WHERE articleID=?1 ORDER BY relatedArticleID",turso::params![id]).await?;let mut related_article_ids=Vec::new();while let Some(row)=rows.next().await?{related_article_ids.push(row.get(0)?);}
            let end=offset.saturating_add(entries.len());Ok(DiscoveryProvenancePage{entries,total,next_offset:if end<total{Some(end)}else{None},related_article_ids})
        }.await;
        match result {
            Ok(page) => {
                tx.commit().await?;
                Ok(page)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
    pub(crate) async fn preview_discovery_import(
        &self,
        records: Vec<DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
    ) -> Result<DiscoveryImportPreview> {
        if records.is_empty() || records.len() > 100 {
            return Err(Error::InvalidSearchQuery(
                "preview requires 1 to 100 candidates".into(),
            ));
        }
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result=async {
            let (library,editable)=target(&tx,library,collection.as_deref()).await?;
            let wanted=records.iter().flat_map(|r|identity_keys(&r.selected.article)).collect();let index=load_index(&tx,library,&wanted).await?;let mut items=Vec::new();
            for record in records {
                let mut item=DiscoveryLibraryMatch{candidate_id:record.candidate_id.clone(),existing:Vec::new(),related_existing:Vec::new(),differences:Vec::new(),error:None};
                match validate_record(&record).map(|_|()) {Err(error)=>item.error=Some(error.to_string()),Ok(())=>match matches(&tx,&index,&record,library).await {
                    Ok((existing,related))=>{
                        if let Some(local)=existing.first(){item.differences=differences(&record,local);}
                        if existing.len()>1{item.error=Some("multiple existing library records match; review duplicates before importing".into());}
                        item.existing=existing;item.related_existing=related;
                    },Err(error)=>item.error=Some(error.to_string()),}}
                items.push(item);
            }
            Ok(DiscoveryImportPreview{library_id:library,collection_id:collection,editable,items})
        }.await;
        match result {
            Ok(preview) => {
                tx.commit().await?;
                Ok(preview)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
    pub(crate) async fn import_discovery(
        &self,
        records: Vec<DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
    ) -> Result<Vec<DiscoveryImportOutcome>> {
        if records.is_empty() || records.len() > 100 {
            return Err(Error::InvalidSearchQuery(
                "import requires 1 to 100 candidates".into(),
            ));
        }
        let wanted: HashSet<String> = records
            .iter()
            .flat_map(|r| identity_keys(&r.selected.article))
            .collect();
        let conn = self.write_conn();
        let mut cache: Option<Cache> = None;
        let mut outcomes = Vec::new();
        for record in records {
            let candidate_id = record.candidate_id.clone();
            let result=async {
                let payload=validate_record(&record)?;
                let tx=Transaction::new_unchecked(&conn,TransactionBehavior::Immediate).await?;
                let result=async {
                    let (library,editable)=target(&tx,library,collection.as_deref()).await?;
                    if !editable{return Err(Error::CollectionConflict("target library is read-only or archived".into()));}
                    let current=marker(&tx).await?;
                    if cache.as_ref().is_none_or(|cache|cache.marker!=current){cache=Some(Cache{marker:current,index:load_index(&tx,library,&wanted).await?});}
                    let (existing,related)=matches(&tx,&cache.as_ref().unwrap().index,&record,library).await?;
                    if existing.len()>1{return Err(Error::SearchConflict("multiple existing library records match; review duplicates before importing".into()));}
                    let (article,status)=if let Some(existing)=existing.into_iter().next(){(existing,DiscoveryImportStatus::AlreadyPresent)}else{
                        let mut article=record.selected.article.clone();article.id=canonical_id(&record,library);article.created_at=Some(Utc::now());article.updated_at=article.created_at;
                        if !BibCore::hydrate_article_ids_on(&tx,std::slice::from_ref(&article.id)).await?.is_empty(){return Err(Error::SearchConflict("generated import identity collides with an unrelated record".into()));}
                        Self::upsert_article_in_tx(&tx,&article).await?;crate::zotero_model::ensure_article_item_in_library(&tx,&article.id,library).await?;crate::zotero_model::sync_article_item(&tx,&article.id).await?;crate::bib_base::sync_search_index(&tx,&article.id).await?;
                        (article,DiscoveryImportStatus::Imported)
                    };
                    let mut collection_added=false;
                    if let Some(collection)=collection.as_deref(){
                        let mut rows=tx.query("SELECT 1 FROM collection_articles WHERE collection_id=?1 AND article_id=?2",turso::params![collection,article.id.clone()]).await?;let exists=rows.next().await?.is_some();drop(rows);
                        if !exists {Self::add_to_collection_in_tx(&tx,collection,&article.id,ArticleRole::Referenced,AddedBy::User,None).await?;collection_added=true;}
                    }
                    let key=format!("{:x}",Sha256::digest(serde_json::to_vec(&(&record.session_id,&record.candidate_id,&record.fingerprint,&record.selected,library))?));
                    let provenance_added=tx.execute("INSERT OR IGNORE INTO discoveryImportProvenance(importKey,articleID,selectedSource,sourceRecordID,payload,importedAt) VALUES (?1,?2,?3,?4,?5,?6)",turso::params![key,article.id.clone(),record.selected.source.clone(),record.selected.article.id.clone(),payload,Utc::now().to_rfc3339()]).await?>0;
                    let keys=identity_keys(&record.selected.article);let mut identities_added=false;
                    for key in &keys {identities_added|=tx.execute("INSERT OR IGNORE INTO discoveryImportIdentities(articleID,identityKey) VALUES (?1,?2)",turso::params![article.id.clone(),key.clone()]).await?>0;}
                    let related_ids=related.into_iter().map(|a|a.id).collect::<Vec<_>>();
                    let mut links_added=false;
                    for related in &related_ids {for (a,b) in [(&article.id,related),(related,&article.id)]{links_added|=tx.execute("INSERT OR IGNORE INTO discoveryItemLinks(articleID,relatedArticleID,relation) VALUES (?1,?2,'possible_version')",turso::params![a.clone(),b.clone()]).await?>0;}}
                    if provenance_added||identities_added||links_added {tx.execute("UPDATE bib_meta SET value=CAST(value AS INTEGER)+1 WHERE key='discovery_identity_revision'",()).await?;tx.execute("UPDATE bib_meta SET value=?1 WHERE key='discovery_identity_token'",turso::params![uuid::Uuid::new_v4().to_string()]).await?;}
                    let next_marker=marker(&tx).await?;
                    Ok((DiscoveryImportOutcome{candidate_id:candidate_id.clone(),status,article:Some(article),related_article_ids:related_ids,collection_added,changed:status==DiscoveryImportStatus::Imported||collection_added||provenance_added||identities_added||links_added,error:None},keys,next_marker))
                }.await;
                match result {
                    Ok((outcome,keys,next_marker))=>{tx.commit().await?;let cache=cache.as_mut().unwrap();for key in keys{index_insert(&mut cache.index,key,outcome.article.as_ref().unwrap().id.clone());}cache.marker=next_marker;Ok(outcome)},
                    Err(error)=>{let _=tx.rollback().await;Err(error)}
                }
            }.await;
            outcomes.push(result.unwrap_or_else(|error| DiscoveryImportOutcome {
                candidate_id,
                status: DiscoveryImportStatus::Failed,
                article: None,
                related_article_ids: Vec::new(),
                collection_added: false,
                changed: false,
                error: Some(error.to_string()),
            }));
        }
        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared(id: &str, title: &str, doi: Option<&str>) -> DiscoveryImportRecord {
        let mut article = Article::new(id, title);
        if let Some(doi) = doi {
            article.identifiers.push(Identifier::doi(doi));
        }
        let selected = DiscoveryRecord {
            source: "fixture".into(),
            article,
            source_rank: 1,
            query_fingerprint: "query-fingerprint".into(),
            retrieved_at: "2026-10-28T00:00:00Z".into(),
            executed_parameters: Default::default(),
            query_translation: None,
        };
        let mut record = DiscoveryImportRecord {
            schema_version: 1,
            session_id: uuid::Uuid::new_v4().to_string(),
            candidate_id: uuid::Uuid::new_v4().to_string(),
            fingerprint: String::new(),
            selected: selected.clone(),
            records: vec![selected],
            request: DiscoveryRequest {
                query: "gene".into(),
                ..Default::default()
            },
            plans: Vec::new(),
        };
        refresh(&mut record);
        record
    }
    fn refresh(record: &mut DiscoveryImportRecord) {
        record.records = vec![record.selected.clone()];
        record.fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&record.records).unwrap())
        );
    }
    #[tokio::test]
    async fn existing_metadata_collection_role_and_repeated_import_are_preserved() {
        let db = BibBase::open_in_memory().await.unwrap();
        let mut local = Article::new("curated", "Locally corrected title");
        local
            .identifiers
            .push(Identifier::doi("https://doi.org/10.1000/RESULT"));
        db.upsert_article(&local).await.unwrap();
        db.upsert_collection(&bib_types::Collection::new("review", "Review"))
            .await
            .unwrap();
        db.add_to_collection(
            "review",
            "curated",
            ArticleRole::Cited,
            AddedBy::User,
            Some("keep this note"),
        )
        .await
        .unwrap();
        let remote = prepared("remote", "Different upstream title", Some("10.1000/result"));
        let preview = db
            .preview_discovery_import(vec![remote.clone()], None, Some("review".into()))
            .await
            .unwrap();
        assert_eq!(preview.items[0].existing[0].id, "curated");
        assert!(
            preview.items[0]
                .differences
                .iter()
                .any(|field| field.field == "title")
        );
        let mut changes = db.subscribe_collection_changes();
        let result = db
            .import_discovery(vec![remote.clone()], None, Some("review".into()))
            .await
            .unwrap();
        assert_eq!(result[0].status, DiscoveryImportStatus::AlreadyPresent);
        assert!(!result[0].collection_added);
        assert_eq!(
            result[0].article.as_ref().unwrap().title,
            "Locally corrected title"
        );
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        let repeated = db
            .import_discovery(vec![remote], None, Some("review".into()))
            .await
            .unwrap();
        assert!(!repeated[0].changed);
        assert!(!changes.has_changed().unwrap());
        assert_eq!(db.article_count().await.unwrap(), 1);
        let membership = db
            .list_collection_articles("review", None, None)
            .await
            .unwrap();
        assert_eq!(membership[0].role, ArticleRole::Cited);
        assert_eq!(membership[0].note.as_deref(), Some("keep this note"));
        let provenance = db.discovery_provenance("curated", 0, 20).await.unwrap();
        assert_eq!(provenance.total, 1);
        assert_eq!(
            provenance.entries[0].provenance.selected.article.title,
            "Different upstream title"
        );
    }
    #[tokio::test]
    async fn a_failed_item_rolls_back_its_article_and_native_rows_but_peers_commit() {
        let core = BibCore::open(":memory:").await.unwrap();
        // The failing item's abstract reaches a real FK failure after its
        // article, identifiers and native root item have been inserted.
        core.write_conn()
            .execute(
                "DELETE FROM fields WHERE fieldID=?1",
                turso::params![crate::zotero_model::FIELD_ABSTRACT],
            )
            .await
            .unwrap();
        let good = prepared("one", "First", Some("10.1000/one"));
        let mut bad = prepared("bad", "Bad", Some("10.1000/bad"));
        bad.selected.article.abstract_text = Some("cannot persist this native field".into());
        refresh(&mut bad);
        let last = prepared("three", "Third", Some("10.1000/three"));
        let result = core
            .import_discovery(vec![good, bad.clone(), last], None, None)
            .await
            .unwrap();
        assert_eq!(
            result.iter().map(|r| r.status).collect::<Vec<_>>(),
            vec![
                DiscoveryImportStatus::Imported,
                DiscoveryImportStatus::Failed,
                DiscoveryImportStatus::Imported
            ]
        );
        assert_eq!(core.article_count().await.unwrap(), 2);
        assert!(
            core.get_article(&canonical_id(&bad, 1))
                .await
                .unwrap()
                .is_none()
        );
        let mut rows = core
            .conn()
            .query("SELECT COUNT(*) FROM articleItems", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );
        assert_eq!(
            core.discovery_provenance(result[0].article.as_ref().unwrap().id.as_str(), 0, 20)
                .await
                .unwrap()
                .total,
            1
        );
    }
    #[tokio::test]
    async fn conflicting_existing_duplicates_are_not_silently_chosen() {
        let db = BibBase::open_in_memory().await.unwrap();
        for id in ["a", "b"] {
            let mut article = Article::new(id, id);
            article.identifiers.push(Identifier::doi("10.1000/same"));
            db.upsert_article(&article).await.unwrap();
        }
        let record = prepared("upstream", "New metadata", Some("10.1000/same"));
        let preview = db
            .preview_discovery_import(vec![record.clone()], None, None)
            .await
            .unwrap();
        assert_eq!(preview.items[0].existing.len(), 2);
        assert!(preview.items[0].error.is_some());
        let result = db.import_discovery(vec![record], None, None).await.unwrap();
        assert_eq!(result[0].status, DiscoveryImportStatus::Failed);
        assert_eq!(db.article_count().await.unwrap(), 2);
    }
    #[tokio::test]
    async fn publication_versions_get_separate_items_and_durable_review_links() {
        let db = BibBase::open_in_memory().await.unwrap();
        let mut preprint = prepared("preprint", "Preprint", Some("10.1000/preprint"));
        preprint.selected.article.source = bib_types::ArticleSource::Arxiv;
        preprint
            .selected
            .article
            .identifiers
            .push(Identifier::new(IdKind::Arxiv, "2201.12345v1"));
        refresh(&mut preprint);
        let first = db
            .import_discovery(vec![preprint], None, None)
            .await
            .unwrap()[0]
            .article
            .as_ref()
            .unwrap()
            .id
            .clone();
        let mut formal = prepared("formal", "Publication", Some("10.1000/publication"));
        formal.selected.article.pub_types = vec!["Journal Article".into()];
        formal
            .selected
            .article
            .identifiers
            .push(Identifier::new(IdKind::Arxiv, "2201.12345v2"));
        refresh(&mut formal);
        let preview = db
            .preview_discovery_import(vec![formal.clone()], None, None)
            .await
            .unwrap();
        assert!(preview.items[0].existing.is_empty());
        assert_eq!(preview.items[0].related_existing[0].id, first);
        let result = db.import_discovery(vec![formal], None, None).await.unwrap();
        assert_eq!(result[0].status, DiscoveryImportStatus::Imported);
        let second = result[0].article.as_ref().unwrap().id.clone();
        assert_ne!(first, second);
        assert_eq!(db.article_count().await.unwrap(), 2);
        assert_eq!(
            db.discovery_provenance(&first, 0, 20)
                .await
                .unwrap()
                .related_article_ids,
            vec![second.clone()]
        );
        assert_eq!(
            db.discovery_provenance(&second, 0, 20)
                .await
                .unwrap()
                .related_article_ids,
            vec![first.clone()]
        );
        db.delete_article(&first).await.unwrap();
        assert!(
            db.discovery_provenance(&second, 0, 20)
                .await
                .unwrap()
                .related_article_ids
                .is_empty()
        );
    }
    #[tokio::test]
    async fn library_scope_is_preserved_and_read_only_libraries_reject_writes() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.write_conn()
            .execute(
                "INSERT INTO libraries(libraryID,type) VALUES (2,'group')",
                (),
            )
            .await
            .unwrap();
        let mut col = bib_types::Collection::new("group", "Group");
        col.library_id = 2;
        core.upsert_collection(&col).await.unwrap();
        let record = prepared("upstream", "Scoped publication", Some("10.1000/scoped"));
        let one = core
            .import_discovery(vec![record.clone()], Some(1), None)
            .await
            .unwrap();
        let two = core
            .import_discovery(vec![record.clone()], None, Some("group".into()))
            .await
            .unwrap();
        assert_eq!(two[0].status, DiscoveryImportStatus::Imported);
        assert_ne!(
            one[0].article.as_ref().unwrap().id,
            two[0].article.as_ref().unwrap().id
        );
        assert!(two[0].collection_added);
        let preview = core
            .preview_discovery_import(vec![record.clone()], None, Some("group".into()))
            .await
            .unwrap();
        assert_eq!(preview.library_id, 2);
        assert_eq!(preview.items[0].existing.len(), 1);
        let mut rows=core.conn().query("SELECT i.libraryID FROM articleItems ai JOIN items i USING(itemID) WHERE ai.article_id=?1",turso::params![two[0].article.as_ref().unwrap().id.clone()]).await.unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );
        drop(rows);
        core.write_conn()
            .execute("UPDATE libraries SET editable=0 WHERE libraryID=2", ())
            .await
            .unwrap();
        assert!(
            !core
                .preview_discovery_import(vec![record], Some(2), None)
                .await
                .unwrap()
                .editable
        );
        let result = core
            .import_discovery(
                vec![prepared("new", "New", Some("10.1000/new"))],
                Some(2),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result[0].status, DiscoveryImportStatus::Failed);
        assert_eq!(core.article_count().await.unwrap(), 2);
    }
    #[tokio::test]
    async fn aliases_from_provenance_match_without_overwriting_curated_identifiers() {
        let db = BibBase::open_in_memory().await.unwrap();
        let mut one = prepared("same-source-id", "First", None);
        one.selected
            .article
            .identifiers
            .push(Identifier::pmid("101"));
        refresh(&mut one);
        let id = db
            .import_discovery(vec![one.clone()], None, None)
            .await
            .unwrap()[0]
            .article
            .as_ref()
            .unwrap()
            .id
            .clone();
        let mut enriched = one;
        enriched.session_id = uuid::Uuid::new_v4().to_string();
        enriched
            .selected
            .article
            .identifiers
            .push(Identifier::doi("10.1000/alias"));
        refresh(&mut enriched);
        assert_eq!(
            db.import_discovery(vec![enriched], None, None)
                .await
                .unwrap()[0]
                .status,
            DiscoveryImportStatus::AlreadyPresent
        );
        let mut third = prepared("other-provider-id", "Third", Some("10.1000/alias"));
        third.selected.source = "other".into();
        refresh(&mut third);
        assert_eq!(
            db.import_discovery(vec![third], None, None).await.unwrap()[0]
                .article
                .as_ref()
                .unwrap()
                .id,
            id
        );
        assert_eq!(db.article_count().await.unwrap(), 1);
        assert!(db.get_article(&id).await.unwrap().unwrap().doi().is_none());
        assert_eq!(db.discovery_provenance(&id, 0, 20).await.unwrap().total, 3);
    }
    #[tokio::test]
    async fn distinct_source_variants_have_distinct_audit_records_and_fingerprints_are_checked() {
        let db = BibBase::open_in_memory().await.unwrap();
        let mut one = prepared("record", "First variant", Some("10.1000/variants"));
        let mut two = one.selected.clone();
        two.article.title = "Second variant".into();
        one.records.push(two.clone());
        one.fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&one.records).unwrap())
        );
        let mut alternate = one.clone();
        alternate.selected = two;
        let id = db
            .import_discovery(vec![one.clone(), alternate], None, None)
            .await
            .unwrap()[0]
            .article
            .as_ref()
            .unwrap()
            .id
            .clone();
        assert_eq!(db.discovery_provenance(&id, 0, 20).await.unwrap().total, 2);
        assert_eq!(
            db.get_article(&id).await.unwrap().unwrap().title,
            "First variant"
        );
        one.fingerprint = "stale".into();
        let result = db.import_discovery(vec![one], None, None).await.unwrap();
        assert_eq!(result[0].status, DiscoveryImportStatus::Failed);
    }
    #[tokio::test]
    async fn concurrent_imports_share_identity_and_provenance_survives_reopen() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Directory(
            std::env::temp_dir().join(format!("discovery-import-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir_all(&dir.0).unwrap();
        let file = dir.0.join("library.db");
        let path = file.to_str().unwrap();
        let a = BibBase::open(path).await.unwrap();
        let b = BibBase::open(path).await.unwrap();
        let one = prepared("a", "Source A", Some("10.1000/concurrent"));
        let mut two = prepared("b", "Source B", Some("10.1000/concurrent"));
        two.selected.source = "other".into();
        refresh(&mut two);
        let (x, y) = tokio::join!(
            a.import_discovery(vec![one], None, None),
            b.import_discovery(vec![two], None, None)
        );
        let x = x.unwrap();
        let y = y.unwrap();
        assert_eq!(
            [x[0].status, y[0].status]
                .iter()
                .filter(|s| **s == DiscoveryImportStatus::Imported)
                .count(),
            1
        );
        let id = x[0].article.as_ref().unwrap().id.clone();
        assert_eq!(y[0].article.as_ref().unwrap().id, id);
        assert_eq!(a.article_count().await.unwrap(), 1);
        let title = if x[0].status == DiscoveryImportStatus::Imported {
            "Source A"
        } else {
            "Source B"
        };
        assert_eq!(a.get_article(&id).await.unwrap().unwrap().title, title);
        drop(a);
        drop(b);
        let reopened = BibBase::open(path).await.unwrap();
        assert_eq!(
            reopened
                .discovery_provenance(&id, 0, 1)
                .await
                .unwrap()
                .total,
            2
        );
        assert!(
            reopened
                .discovery_provenance(&id, 0, 1)
                .await
                .unwrap()
                .next_offset
                .is_some()
        );
    }
}
