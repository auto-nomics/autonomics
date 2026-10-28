//! Snapshot-consistent local search and replay of the database outbox.
use crate::{
    BibBase, Error, Result,
    actor::BibCommand,
    bib_base::{
        BibCore, SearchCandidate, SearchEngineMode, extract_snippet_from_text, unique_query_terms,
    },
    local_index::SourceDocument,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_types::{SearchExpr, SearchHit, SearchMode, SearchPageV2, SearchRequestV2, SearchSort};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use turso::{
    Value,
    transaction::{Transaction, TransactionBehavior},
};

/// Search owns a separate read connection. Its long replay/query work cannot
/// hold the storage actor's connection or delay metadata writes and status.
pub(crate) enum SearchJob {
    Export {
        request: SearchRequestV2,
        prepared: Option<crate::semantic_service::PreparedSemanticQuery>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<bib_types::Article>>>,
    },
    Evidence {
        prepared: Option<crate::semantic_service::PreparedSemanticQuery>,
        article_id: String,
        input: bib_types::SearchEvidenceRequest,
        reply: tokio::sync::oneshot::Sender<Result<bib_types::SearchEvidencePage>>,
    },
    V2 {
        request: SearchRequestV2,
        prepared: Option<crate::semantic_service::PreparedSemanticQuery>,
        reply: tokio::sync::oneshot::Sender<Result<SearchPageV2>>,
    },
    Legacy {
        query: String,
        limit: usize,
        offset: usize,
        reply: tokio::sync::oneshot::Sender<Result<Vec<SearchHit>>>,
    },
    Collection {
        input: crate::collections::CollectionQuery,
        reply: tokio::sync::oneshot::Sender<Result<crate::collections::CollectionPage>>,
    },
    Rebuild {
        reply: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    },
}
impl SearchJob {
    pub fn reject(self) {
        match self {
            Self::Export { reply, .. } => {
                let _ = reply.send(Err(Error::ActorStopped));
            }
            Self::Evidence { reply, .. } => {
                let _ = reply.send(Err(Error::ActorStopped));
            }
            Self::V2 { reply, .. } => {
                let _ = reply.send(Err(Error::ActorStopped));
            }
            Self::Legacy { reply, .. } => {
                let _ = reply.send(Err(Error::ActorStopped));
            }
            Self::Collection { reply, .. } => {
                let _ = reply.send(Err(Error::ActorStopped));
            }
            Self::Rebuild { reply } => {
                if let Some(reply) = reply {
                    let _ = reply.send(Err(Error::ActorStopped));
                }
            }
        }
    }
    async fn run(self, core: &BibCore) {
        match self {
            Self::Export {
                request,
                prepared,
                reply,
            } => {
                if !reply.is_closed() {
                    let _ = reply.send(core.export_search_prepared(request, prepared).await);
                }
            }
            Self::Evidence {
                prepared,
                article_id,
                input,
                reply,
            } => {
                if !reply.is_closed() {
                    let _ = reply.send(
                        core.search_evidence_prepared(&article_id, input, prepared)
                            .await,
                    );
                }
            }
            Self::V2 {
                request,
                prepared,
                reply,
            } => {
                if !reply.is_closed() {
                    let _ = reply.send(core.search_v2_prepared(request, prepared).await);
                }
            }
            Self::Legacy {
                query,
                limit,
                offset,
                reply,
            } => {
                if !reply.is_closed() {
                    let _ = reply.send(core.search_articles(&query, limit, offset).await);
                }
            }
            Self::Collection { input, reply } => {
                if !reply.is_closed() {
                    let _ = reply.send(core.query_collection_articles(input).await);
                }
            }
            Self::Rebuild { reply } => {
                if let Ok(mut index) = core.search_index.lock() {
                    index.revision = None;
                }
                let result = core.refresh_search_index().await;
                if let Err(error) = &result {
                    core.record_index_error(error);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
        }
    }
}
pub(crate) async fn run_search_service(
    core: BibCore,
    mut rx: tokio::sync::mpsc::Receiver<SearchJob>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            job=rx.recv() => match job { Some(job)=>job.run(&core).await, None=>break },
            _=tick.tick() => if let Err(error)=core.refresh_search_index().await {
                core.record_index_error(&error);
                tracing::warn!(%error,"bibliography index replay failed; will retry");
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    version: u8,
    fingerprint: String,
    offset: usize,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSearchQuery(message.into())
}
fn task_error(error: tokio::task::JoinError) -> Error {
    Error::Unknown(format!("search worker: {error}"))
}

impl BibBase {
    pub(crate) async fn prepare_search(
        &self,
        request: &SearchRequestV2,
    ) -> Result<Option<crate::semantic_service::PreparedSemanticQuery>> {
        let request = validate_request(request.clone())?;
        if request.mode == SearchMode::Lexical {
            return Ok(None);
        }
        let service = self
            .semantic
            .as_ref()
            .ok_or_else(|| invalid("semantic search requires the Tantivy engine"))?;
        service
            .prepare(request.semantic_query.as_deref().expect("validated topic"))
            .await
            .map(Some)
    }
    pub async fn search_index_status(&self) -> Result<bib_types::SearchIndexStatus> {
        self.call(|reply| BibCommand::SearchIndexStatus { reply })
            .await
    }
    /// Queue a rebuild without blocking the storage actor or HTTP caller.
    pub async fn request_search_rebuild(&self) -> Result<()> {
        self.call(|reply| BibCommand::RequestSearchRebuild { reply })
            .await
    }
    pub async fn rebuild_search_index(&self) -> Result<()> {
        self.call(|reply| BibCommand::RebuildSearchIndex { reply })
            .await
    }
    pub async fn export_search(&self, request: SearchRequestV2) -> Result<Vec<bib_types::Article>> {
        let prepared = self.prepare_search(&request).await?;
        self.call(|reply| BibCommand::ExportSearch {
            request,
            prepared,
            reply,
        })
        .await
    }
    pub async fn search_v2(&self, request: SearchRequestV2) -> Result<SearchPageV2> {
        let prepared = self.prepare_search(&request).await?;
        self.call(|reply| BibCommand::SearchV2 {
            request,
            prepared,
            reply,
        })
        .await
    }
}

impl BibCore {
    pub(crate) fn mark_build_requested(&self) {
        if let Ok(mut progress) = self.search_progress.lock() {
            progress.building = true;
            progress.processed_documents = 0;
            progress.target_revision = None;
        }
    }
    fn record_index_error(&self, error: &Error) {
        if let Ok(mut progress) = self.search_progress.lock() {
            progress.last_error = Some(error.to_string());
            progress.building = false;
            progress.target_revision = None;
        }
    }
    pub(crate) async fn search_index_status(&self) -> Result<bib_types::SearchIndexStatus> {
        let progress = self
            .search_progress
            .lock()
            .map_err(|_| Error::Unknown("search progress lock poisoned".into()))?
            .clone();
        let (indexed_revision, indexed_documents, durable, last_error) = (
            progress.revision,
            progress.documents,
            progress.durable,
            progress.last_error.clone(),
        );
        let conn = self.conn();
        let mut rows = conn
            .query("SELECT value FROM bib_meta WHERE key='search_revision'", ())
            .await?;
        let source_revision = match rows.next().await? {
            Some(row) => row
                .get::<String>(0)?
                .parse::<u64>()
                .map_err(|_| Error::Unknown("invalid search revision".into()))?,
            None => 0,
        };
        drop(rows);
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM searchOutbox WHERE revision > ?1",
                turso::params![indexed_revision.unwrap_or(0) as i64],
            )
            .await?;
        let pending_articles = rows
            .next()
            .await?
            .ok_or_else(|| Error::Unknown("index count returned no row".into()))?
            .get::<i64>(0)?
            .max(0) as usize;
        Ok(bib_types::SearchIndexStatus {
            engine: if self.search_engine == SearchEngineMode::Legacy {
                "legacy"
            } else {
                "tantivy"
            }
            .into(),
            source_revision,
            indexed_revision,
            pending_articles,
            indexed_documents,
            durable,
            last_error,
            building: progress.building,
            processed_documents: progress.processed_documents,
            target_revision: progress.target_revision,
        })
    }
    /// Called by the idle actor and before each search. Outbox events are kept
    /// per article, rather than globally acknowledged, so every process owns
    /// an independent replay cursor (the durable index stores it in its commit).
    pub(crate) async fn refresh_search_index(&self) -> Result<()> {
        if self.search_engine == SearchEngineMode::Legacy {
            return Ok(());
        }
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = self.refresh_index_snapshot().await;
        match result {
            Ok(()) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    async fn refresh_index_snapshot(&self) -> Result<()> {
        let (indexed, indexed_token, source_checked) = {
            let index = self
                .search_index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?;
            (
                index.revision,
                index.commit_token.clone(),
                index.source_checked,
            )
        };
        let conn = self.conn();
        let mut rows = conn
            .query("SELECT value FROM bib_meta WHERE key='search_revision'", ())
            .await?;
        let revision = match rows.next().await? {
            Some(row) => row
                .get::<String>(0)?
                .parse::<u64>()
                .map_err(|_| Error::Unknown("invalid search revision".into()))?,
            None => 0,
        };
        drop(rows);
        let mut rows = conn
            .query(
                "SELECT value FROM bib_meta WHERE key='search_revision_token'",
                (),
            )
            .await?;
        let commit_token = rows
            .next()
            .await?
            .ok_or_else(|| {
                Error::Unknown("search commit token missing; reopen the library to migrate".into())
            })?
            .get::<String>(0)?;
        drop(rows);
        if indexed == Some(revision) && indexed_token.as_deref() == Some(&commit_token) {
            self.search_index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                .source_checked = true;
            return Ok(());
        }
        // Counters can repeat after a point-in-time restore. A fresh mutation
        // token distinguishes divergent histories at the same revision.
        // A freshly opened sidecar cannot establish ancestry of a changed
        // source, even when the restored database has a higher revision.
        let rebuild = !source_checked
            || indexed.is_none_or(|indexed| indexed >= revision)
            || indexed_token.is_none();
        let mut articles = Vec::new();
        if !rebuild {
            let mut rows = conn
                .query(
                    "SELECT articleID FROM searchOutbox WHERE revision > ?1 ORDER BY revision",
                    turso::params![indexed.unwrap_or(0) as i64],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                articles.push(row.get::<String>(0)?);
            }
        }
        // The event projection is atomic with canonical documents. A missing
        // event on revision mismatch indicates an older database or restore;
        // rebuilding is safer than claiming the index has caught up.
        let rebuild = rebuild || articles.is_empty();
        let mut documents = Vec::new();
        let select = "SELECT docID,articleID,anchorKey,kind,title,abstract,creatorText,publicationText,identifierText,keywordText,tagText,yearText,body,identifierExact FROM searchDocuments";
        let batches = if rebuild {
            vec![Vec::new()]
        } else {
            articles.chunks(300).map(|chunk| chunk.to_vec()).collect()
        };
        let index = self.search_index.clone();
        tokio::task::spawn_blocking(move || {
            index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                .begin_update(articles, revision, commit_token, rebuild)
        })
        .await
        .map_err(task_error)??;
        let build_result: Result<()> = async {
            let mut batch_bytes = 0;
            for batch in batches {
                let (sql, params) = if rebuild {
                    (select.to_owned(), Vec::new())
                } else {
                    let placeholders = (1..=batch.len())
                        .map(|i| format!("?{i}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    let projection=crate::search_projection::select(Some(&format!("a.id IN ({placeholders})")));
                    (
                        format!("SELECT docID,articleID,anchorKey,kind,title,abstract,creatorText,publicationText,identifierText,keywordText,tagText,yearText,body,identifierExact FROM ({projection}) p"),
                        batch.into_iter().map(Value::Text).collect(),
                    )
                };
                let mut rows = conn.query(sql, turso::params_from_iter(params)).await?;
                while let Some(row) = rows.next().await? {
                    let kind = row.get::<String>(3)?;
                    let body = row.get::<String>(12)?;
                    let year = row.get::<String>(11)?;
                    documents.push(SourceDocument {
                        meta: SearchCandidate {
                            doc_id: row.get(0)?,
                            article_id: row.get(1)?,
                            anchor_key: row.get(2)?,
                            kind: kind.clone(),
                            title: row.get(4)?,
                            year: year.parse().ok(),
                            score: 0.0,
                            matched_field: "title".into(),
                            snippet_query: String::new(),
                        },
                        texts: [
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                            row.get(10)?,
                            year,
                            if kind == "annotation" {
                                body.clone()
                            } else {
                                String::new()
                            },
                            if kind == "attachment" {
                                body
                            } else {
                                String::new()
                            },
                        ],
                        identifiers: serde_json::from_str(&row.get::<String>(13)?)?,
                    });
                    batch_bytes += documents
                        .last()
                        .map(|doc| doc.texts.iter().map(String::len).sum::<usize>())
                        .unwrap_or(0);
                    if documents.len() >= 128 || batch_bytes >= 4 * 1024 * 1024 {
                        self.append_index_batch(std::mem::take(&mut documents))
                            .await?;
                        batch_bytes = 0;
                    }
                }
            }
            if !documents.is_empty() {
                self.append_index_batch(documents).await?;
            }
            let index = self.search_index.clone();
            tokio::task::spawn_blocking(move || {
                index
                    .lock()
                    .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                    .finish_update()
            })
            .await
            .map_err(task_error)??;
            Ok(())
        }
        .await;
        if let Err(error) = build_result {
            let index = self.search_index.clone();
            let _ = tokio::task::spawn_blocking(move || {
                index
                    .lock()
                    .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                    .abort_update()
            })
            .await;
            return Err(error);
        }
        Ok(())
    }

    async fn append_index_batch(&self, documents: Vec<SourceDocument>) -> Result<()> {
        let index = self.search_index.clone();
        tokio::task::spawn_blocking(move || {
            index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                .append_documents(documents)
        })
        .await
        .map_err(task_error)?
    }

    pub(crate) async fn ranked_local(
        &self,
        expression: Option<SearchExpr>,
        same_child: bool,
    ) -> Result<Vec<SearchCandidate>> {
        let index = self.search_index.clone();
        tokio::task::spawn_blocking(move || {
            index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                .search(expression.as_ref(), same_child)
        })
        .await
        .map_err(task_error)?
    }

    pub(crate) async fn ranked_local_in(
        &self,
        expression: SearchExpr,
        allowed: std::sync::Arc<HashSet<String>>,
        limit: usize,
    ) -> Result<Vec<SearchCandidate>> {
        let index = self.search_index.clone();
        tokio::task::spawn_blocking(move || {
            index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
                .search_top_in(&expression, &allowed, limit)
        })
        .await
        .map_err(task_error)?
    }

    /// Called inside the caller's read transaction so ranking, scope,
    /// hydration, and snippets share one source snapshot.
    pub(crate) async fn local_matches_snapshot(&self, query: &str) -> Result<Vec<SearchCandidate>> {
        if self.search_engine == SearchEngineMode::Legacy {
            crate::legacy_index::refresh(&self.conn()).await?;
            return self.legacy_candidates(query).await;
        }
        // Preserve the old endpoint's empty result for punctuation-only
        // input, including SQL wildcard characters. V2 reports invalid ASTs.
        if !query.trim().is_empty() && unique_query_terms(query).is_empty() {
            return Ok(Vec::new());
        }
        let request = validate_request(SearchRequestV2 {
            query: query.into(),
            ..Default::default()
        })?;
        let (mut hits, _) = self.scoped_search_candidates(&request).await?;
        if query.trim().is_empty() {
            sort_candidates(&mut hits, SearchSort::Title);
        }
        Ok(hits)
    }

    async fn legacy_candidates(&self, query: &str) -> Result<Vec<SearchCandidate>> {
        let conn = self.conn();
        let terms = unique_query_terms(query);
        if terms.len() > 128 {
            return Err(invalid("queries may contain at most 128 distinct terms"));
        }
        if !query.trim().is_empty() && terms.is_empty() {
            return Ok(Vec::new());
        }
        if terms.is_empty() {
            let mut rows = conn.query("SELECT a.id,a.title,a.year,i.key FROM articles a JOIN articleItems m ON m.article_id=a.id JOIN items i ON i.itemID=m.itemID ORDER BY a.title COLLATE NOCASE,a.id", ()).await?;
            let mut hits = Vec::new();
            while let Some(row) = rows.next().await? {
                let anchor_key = row.get::<String>(3)?;
                hits.push(SearchCandidate {
                    doc_id: format!("{anchor_key}:item"),
                    article_id: row.get(0)?,
                    anchor_key,
                    kind: "item".into(),
                    title: row.get(1)?,
                    year: row.get::<i64>(2).ok(),
                    score: 0.0,
                    matched_field: "title".into(),
                    snippet_query: query.into(),
                });
            }
            return Ok(hits);
        }
        let placeholders = (1..=terms.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT s.articleID,s.anchorKey,s.field,a.title,a.year FROM searchTerms s JOIN articles a ON a.id=s.articleID WHERE s.term IN ({placeholders}) AND s.articleID IN (SELECT articleID FROM searchTerms WHERE term IN ({placeholders}) GROUP BY articleID HAVING COUNT(DISTINCT term)=?{}) GROUP BY s.articleID,s.anchorKey,s.field,a.title,a.year ORDER BY CASE s.field WHEN 'title' THEN 0 WHEN 'abstract' THEN 1 WHEN 'annotation' THEN 2 ELSE 3 END,COUNT(*) DESC,a.id,s.anchorKey",
            terms.len() + 1
        );
        let mut params = terms.into_iter().map(Value::Text).collect::<Vec<_>>();
        params.push(Value::Integer((params.len()) as i64));
        let mut rows = conn.query(sql, turso::params_from_iter(params)).await?;
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        while let Some(row) = rows.next().await? {
            let article_id = row.get::<String>(0)?;
            if !seen.insert(article_id.clone()) {
                continue;
            }
            let anchor_key = row.get::<String>(1)?;
            let field = row.get::<String>(2)?;
            let (kind, score) = match field.as_str() {
                "title" => ("item", 0.0),
                "abstract" => ("item", 1.0),
                "annotation" => ("annotation", 2.0),
                _ => ("attachment", 3.0),
            };
            hits.push(SearchCandidate {
                doc_id: format!("{anchor_key}:{kind}"),
                article_id,
                anchor_key,
                kind: kind.into(),
                title: row.get(3)?,
                year: row.get::<i64>(4).ok(),
                score,
                matched_field: field,
                snippet_query: query.into(),
            });
        }
        Ok(hits)
    }

    pub(crate) async fn search_legacy(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>> {
        let conn = self.conn();
        let behavior = if self.search_engine == SearchEngineMode::Legacy {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let tx = Transaction::new_unchecked(&conn, behavior).await?;
        let result = async {
            let ranked = self.local_matches_snapshot(query).await?;
            let ranked = ranked
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            self.render_search_hits(&ranked, query).await
        }
        .await;
        match result {
            Ok(hits) => {
                tx.commit().await?;
                Ok(hits)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    pub(crate) async fn render_search_hits(
        &self,
        ranked: &[SearchCandidate],
        query: &str,
    ) -> Result<Vec<SearchHit>> {
        let conn = self.conn();
        let mut groups: HashMap<&str, Vec<&SearchCandidate>> = HashMap::new();
        for hit in ranked {
            groups.entry(&hit.matched_field).or_default().push(hit);
        }
        let mut snippets = HashMap::new();
        for (field, hits) in groups {
            let column = match field {
                "title" => "title",
                "abstract" => "abstract",
                "creator" => "creatorText",
                "publication" => "publicationText",
                "identifier" => "identifierText",
                "keyword" => "keywordText",
                "tag" => "tagText",
                "year_text" => "yearText",
                "annotation" | "fulltext" => "body",
                _ => "metadata",
            };
            for chunk in hits.chunks(100) {
                let placeholders = (1..=chunk.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let mut params = chunk
                    .iter()
                    .map(|hit| Value::Text(hit.doc_id.clone()))
                    .collect::<Vec<_>>();
                let article_placeholders = (chunk.len() + 1..=chunk.len() * 2)
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",");
                params.extend(chunk.iter().map(|hit| Value::Text(hit.article_id.clone())));
                let projection = crate::search_projection::select(Some(&format!(
                    "a.id IN ({article_placeholders})"
                )));
                let mut rows = conn.query(format!("SELECT docID,{column} FROM ({projection}) p WHERE docID IN ({placeholders})"), turso::params_from_iter(params)).await?;
                while let Some(row) = rows.next().await? {
                    let text = row.get::<String>(1)?;
                    let text = if field == "keyword" {
                        serde_json::from_str::<Vec<String>>(&text)
                            .map(|values| values.join("\u{1e}"))
                            .unwrap_or(text)
                    } else {
                        text
                    };
                    let doc_id = row.get::<String>(0)?;
                    let matched_query = chunk
                        .iter()
                        .find(|hit| hit.doc_id == doc_id)
                        .map(|hit| hit.snippet_query.as_str())
                        .filter(|query| !query.is_empty())
                        .unwrap_or(query);
                    snippets.insert(
                        row.get::<String>(0)?,
                        extract_snippet_from_text(matched_query, &text)
                            .unwrap_or_else(|| text.chars().take(160).collect())
                            .replace('\u{1e}', " · "),
                    );
                }
            }
        }
        Ok(ranked
            .iter()
            .map(|hit| {
                let snippet = snippets
                    .remove(&hit.doc_id)
                    .unwrap_or_else(|| hit.title.clone());
                let ranges = crate::search_analyzer::highlight_ranges(&hit.snippet_query, &snippet);
                SearchHit {
                    article_id: hit.article_id.clone(),
                    title: hit.title.clone(),
                    score: hit.score,
                    snippet,
                    highlight_ranges: ranges,
                    anchor_key: Some(hit.anchor_key.clone()),
                    matched_field: Some(hit.matched_field.clone()),
                }
            })
            .collect())
    }

    pub(crate) async fn export_search_prepared(
        &self,
        mut request: SearchRequestV2,
        prepared: Option<crate::semantic_service::PreparedSemanticQuery>,
    ) -> Result<Vec<bib_types::Article>> {
        if self.search_engine == SearchEngineMode::Legacy {
            return Err(invalid(
                "V2 export is disabled by the legacy engine rollout flag",
            ));
        }
        request = validate_request(request)?;
        if request.cursor.is_some() {
            return Err(invalid(
                "export requires a complete search without a page cursor",
            ));
        }
        request.include_facets = false;
        request.include_articles = false;
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = async {
            let mut hits = self
                .evaluate_search(&request, prepared.as_ref())
                .await?
                .hits;
            sort_candidates(&mut hits, request.sort);
            let ids = hits
                .into_iter()
                .map(|hit| hit.article_id)
                .collect::<Vec<_>>();
            let articles = self.hydrate_article_ids(&ids).await?;
            if let Some(prepared) = &prepared {
                self.validate_semantic_configuration(prepared).await?;
            }
            Ok::<_, Error>(articles)
        }
        .await;
        match result {
            Ok(articles) => {
                tx.commit().await?;
                Ok(articles)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn search_v2(&self, request: SearchRequestV2) -> Result<SearchPageV2> {
        self.search_v2_prepared(request, None).await
    }
    pub(crate) async fn search_v2_prepared(
        &self,
        mut request: SearchRequestV2,
        prepared: Option<crate::semantic_service::PreparedSemanticQuery>,
    ) -> Result<SearchPageV2> {
        if self.search_engine == SearchEngineMode::Legacy {
            return Err(invalid(
                "V2 search is disabled by the legacy engine rollout flag",
            ));
        }
        request = validate_request(request)?;
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = self.search_v2_snapshot(request, prepared.as_ref()).await;
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

    pub(crate) async fn scoped_search_candidates(
        &self,
        request: &SearchRequestV2,
    ) -> Result<(Vec<SearchCandidate>, Option<bib_types::SearchFacets>)> {
        self.refresh_index_snapshot().await?;
        let mut hits = self
            .ranked_local(request.expression.clone(), request.scope.require_same_child)
            .await?;
        let conn = self.conn();
        if let Some(id) = &request.scope.collection_id {
            let collection = self
                .get_collection(id)
                .await?
                .filter(|collection| !collection.deleted)
                .ok_or_else(|| Error::NotFound(format!("collection {id}")))?;
            let ids = if request.scope.include_descendants {
                crate::collections::subtree_ids(&conn, id)
                    .await?
                    .into_iter()
                    .map(|(_, id)| id)
                    .collect()
            } else {
                vec![collection.collection_id]
            };
            let ids = ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let mut rows = conn.query(format!("SELECT ai.article_id FROM collectionItems ci JOIN articleItems ai USING(itemID) WHERE ci.collectionID IN ({ids}) AND NOT EXISTS (SELECT 1 FROM deletedCollections d WHERE d.collectionID=ci.collectionID)"), ()).await?;
            let mut allowed = HashSet::new();
            while let Some(row) = rows.next().await? {
                allowed.insert(row.get::<String>(0)?);
            }
            hits.retain(|hit| allowed.contains(&hit.article_id));
        } else if request.scope.unfiled {
            let mut rows = conn
                .query("SELECT DISTINCT article_id FROM collection_articles", ())
                .await?;
            let mut filed = HashSet::new();
            while let Some(row) = rows.next().await? {
                filed.insert(row.get::<String>(0)?);
            }
            hits.retain(|hit| !filed.contains(&hit.article_id));
        }
        let facets = crate::search_filters::apply(
            self,
            &mut hits,
            &request.filters,
            request.scope.library_id,
            request.include_facets,
        )
        .await?;
        Ok((hits, facets))
    }

    async fn search_v2_snapshot(
        &self,
        mut request: SearchRequestV2,
        prepared: Option<&crate::semantic_service::PreparedSemanticQuery>,
    ) -> Result<SearchPageV2> {
        let cursor = request.cursor.take();
        let query_bytes = serde_json::to_vec(&request)?;
        let mut evaluation = self.evaluate_search(&request, prepared).await?;
        let mut hits = std::mem::take(&mut evaluation.hits);
        let facets = evaluation.facets;
        sort_candidates(&mut hits, request.sort);
        let (revision, token) = {
            let index = self
                .search_index
                .lock()
                .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?;
            (
                index.revision.unwrap_or(0),
                index.commit_token.clone().unwrap_or_default(),
            )
        };
        let generation = crate::hybrid_search::generation(revision, &token, prepared);
        let mut digest = Sha256::new();
        digest.update(query_bytes);
        digest.update(generation.as_bytes());
        digest.update(serde_json::to_vec(&hits)?);
        let fingerprint = format!("{:x}", digest.finalize());
        let offset = if let Some(cursor) = cursor {
            if cursor.len() > 2048 {
                return Err(invalid("invalid cursor"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| invalid("invalid cursor"))?;
            let cursor: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| invalid("invalid cursor"))?;
            if cursor.version != 1
                || cursor.fingerprint != fingerprint
                || cursor.offset > hits.len()
            {
                return Err(invalid(
                    "search cursor expired or belongs to another query; restart search",
                ));
            }
            cursor.offset
        } else {
            0
        };
        let total = hits.len();
        let end = offset.saturating_add(request.page_size as usize).min(total);
        let next_cursor = if end < total {
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
                version: 1,
                fingerprint,
                offset: end,
            })?))
        } else {
            None
        };
        let (articles, associations) = if request.include_articles {
            let ids = hits[offset..end]
                .iter()
                .map(|hit| hit.article_id.clone())
                .collect::<Vec<_>>();
            (
                self.hydrate_article_ids(&ids).await?,
                self.article_associations(&ids).await?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let text = positive_text(request.expression.as_ref());
        let mut rendered = self.render_search_hits(&hits[offset..end], &text).await?;
        self.render_semantic_page(
            &hits[offset..end],
            &mut rendered,
            &mut evaluation.retrieval,
            &evaluation.semantic_chunks,
        )
        .await?;
        if let Some(prepared) = prepared {
            self.validate_semantic_configuration(prepared).await?;
        }
        Ok(SearchPageV2 {
            hits: rendered,
            articles,
            associations,
            total,
            next_cursor,
            index_generation: generation,
            facets,
            retrieval: evaluation.retrieval,
        })
    }
}
fn sort_candidates(hits: &mut [SearchCandidate], sort: SearchSort) {
    match sort {
        SearchSort::Title => {
            hits.sort_by_cached_key(|hit| (hit.title.to_lowercase(), hit.article_id.clone()))
        }
        SearchSort::Newest | SearchSort::Oldest => hits.sort_by(|a, b| {
            match (a.year, b.year) {
                (Some(a), Some(b)) if sort == SearchSort::Newest => b.cmp(&a),
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
            .then_with(|| a.article_id.cmp(&b.article_id))
        }),
        _ => {}
    }
}

fn apply_default_fields(expr: SearchExpr, fields: &[bib_types::SearchField]) -> Result<SearchExpr> {
    use bib_types::SearchField;
    let phrase = matches!(&expr, SearchExpr::Phrase { .. });
    match expr {
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field }
            if field.is_none() || field == Some(SearchField::Any) =>
        {
            // Preserve positional phrase semantics in each chosen field.
            let mut clauses = Vec::new();
            for field in fields {
                if *field == SearchField::Year && crate::local_index::parse_year(&value).is_err() {
                    continue;
                }
                if clauses.iter().any(|clause| match clause {
                    SearchExpr::Term {
                        field: existing, ..
                    }
                    | SearchExpr::Phrase {
                        field: existing, ..
                    } => *existing == Some(*field),
                    _ => false,
                }) {
                    continue;
                }
                clauses.push(if phrase {
                    SearchExpr::Phrase {
                        value: value.clone(),
                        field: Some(*field),
                    }
                } else {
                    SearchExpr::Term {
                        value: value.clone(),
                        field: Some(*field),
                    }
                });
            }
            if clauses.is_empty() {
                return Err(invalid(
                    "unfielded text is incompatible with the selected default fields",
                ));
            }
            Ok(if clauses.len() == 1 {
                clauses.remove(0)
            } else {
                SearchExpr::Or { clauses }
            })
        }
        SearchExpr::And { clauses } => Ok(SearchExpr::And {
            clauses: clauses
                .into_iter()
                .map(|c| apply_default_fields(c, fields))
                .collect::<Result<_>>()?,
        }),
        SearchExpr::Or { clauses } => Ok(SearchExpr::Or {
            clauses: clauses
                .into_iter()
                .map(|c| apply_default_fields(c, fields))
                .collect::<Result<_>>()?,
        }),
        SearchExpr::Not { clause } => Ok(SearchExpr::Not {
            clause: Box::new(apply_default_fields(*clause, fields)?),
        }),
        other => Ok(other),
    }
}

pub(crate) fn validate_request(mut request: SearchRequestV2) -> Result<SearchRequestV2> {
    if !request.query.trim().is_empty() {
        if request.expression.is_some() {
            return Err(invalid("supply either query or expression"));
        }
        request.expression =
            bib_types::parse_search_query(&request.query).map_err(|e| invalid(e.to_string()))?;
    }
    if !(1..=200).contains(&request.page_size) {
        return Err(invalid("page_size must be between 1 and 200"));
    }
    request.semantic_query = request
        .semantic_query
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    match request.mode {
        SearchMode::Lexical if request.semantic_query.is_some() => {
            return Err(invalid("semantic_query requires hybrid mode"));
        }
        SearchMode::Hybrid => {
            crate::hybrid_search::topic_expression(
                request
                    .semantic_query
                    .as_deref()
                    .ok_or_else(|| invalid("hybrid mode requires semantic_query"))?,
            )?;
        }
        _ => {}
    }
    if request.sources.iter().any(|source| source != "local") {
        return Err(invalid("local search supports only the local source"));
    }
    if request.sort == SearchSort::CitationCount {
        return Err(invalid(
            "citation-count ordering requires external citation data",
        ));
    }
    crate::search_filters::validate(&request.filters)?;
    if request.scope.library_id.is_some_and(|id| id <= 0) {
        return Err(invalid("library_id must be positive"));
    }
    if request.scope.unfiled && request.scope.collection_id.is_some() {
        return Err(invalid("unfiled and collection scope cannot be combined"));
    }
    if request.default_fields.len() > 10
        || request
            .default_fields
            .contains(&bib_types::SearchField::Any)
    {
        return Err(invalid(
            "default_fields must contain at most 10 concrete fields",
        ));
    }
    if !request.default_fields.is_empty() {
        request.expression = request
            .expression
            .map(|expr| apply_default_fields(expr, &request.default_fields))
            .transpose()?;
    }
    if let Some(expr) = &request.expression {
        crate::local_index::validate(expr, 0, &mut 0)?;
    }
    Ok(request)
}
fn positive_text(expr: Option<&SearchExpr>) -> String {
    fn visit(expr: &SearchExpr, positive: bool, values: &mut Vec<String>) {
        match expr {
            SearchExpr::Term { value, .. } | SearchExpr::Phrase { value, .. } if positive => {
                values.push(value.clone())
            }
            SearchExpr::Not { clause } => visit(clause, !positive, values),
            SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
                for clause in clauses {
                    visit(clause, positive, values);
                }
            }
            _ => {}
        }
    }
    let mut values = Vec::new();
    if let Some(expr) = expr {
        visit(expr, true, &mut values);
    }
    values.join(" ")
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use bib_types::Article;

    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn database() -> (Directory, String) {
        let directory =
            Directory(std::env::temp_dir().join(format!("bib-search-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&directory.0).unwrap();
        let path = directory
            .0
            .join("library.db")
            .to_string_lossy()
            .into_owned();
        (directory, path)
    }

    #[tokio::test]
    async fn a_cancelled_queued_search_does_not_replay_or_evaluate() {
        let (_directory, path) = database();
        let core = BibCore::open(&path).await.unwrap();
        core.upsert_article(&Article::new("a", "Queued target"))
            .await
            .unwrap();
        core.search_index.lock().unwrap().revision = None;
        let (reply, receiver) = tokio::sync::oneshot::channel();
        drop(receiver);
        SearchJob::V2 {
            request: SearchRequestV2::default(),
            prepared: None,
            reply,
        }
        .run(&core)
        .await;
        assert!(
            core.search_index.lock().unwrap().revision.is_none(),
            "a cancelled queue entry must never touch the index"
        );
    }

    #[tokio::test]
    async fn higher_revision_backup_rechecks_lexical_projection_after_reopen() {
        let (_directory, path) = database();
        {
            let core = BibCore::open(&path).await.unwrap();
            core.upsert_article(&Article::new("a", "Alpha oldmarker"))
                .await
                .unwrap();
            core.upsert_article(&Article::new("b", "Beta oldmarker"))
                .await
                .unwrap();
            core.refresh_search_index().await.unwrap();
            core.upsert_article(&Article::new("b", "Restoredbranch evidence"))
                .await
                .unwrap();
            core.upsert_article(&Article::new("a", "Latestalpha evidence"))
                .await
                .unwrap();
            core.write_conn()
                .execute("UPDATE searchOutbox SET revision=1 WHERE articleID='b'", ())
                .await
                .unwrap();
        }
        let reopened = BibCore::open(&path).await.unwrap();
        let matches = reopened
            .search_articles("restoredbranch", 10, 0)
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].article_id, "b");
        assert!(
            reopened
                .search_articles("oldmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(reopened.search_index.lock().unwrap().source_checked);
    }

    #[tokio::test]
    async fn pending_events_replay_after_reopen_and_missing_index_rebuilds() {
        let (_directory, path) = database();
        {
            let core = BibCore::open(&path).await.unwrap();
            core.upsert_article(&Article::new("old", "Oldmarker evidence"))
                .await
                .unwrap();
            assert_eq!(
                core.search_articles("oldmarker", 10, 0)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            core.upsert_article(&Article::new("new", "Freshmarker evidence"))
                .await
                .unwrap();
            core.delete_article("old").await.unwrap();
            // Drop without consuming the latest events: the durable commit
            // still contains the old article and no new article.
        }
        {
            let core = BibCore::open(&path).await.unwrap();
            assert_eq!(
                core.search_articles("freshmarker", 10, 0).await.unwrap()[0].article_id,
                "new"
            );
            assert!(
                core.search_articles("oldmarker", 10, 0)
                    .await
                    .unwrap()
                    .is_empty()
            );
            let status = core.search_index_status().await.unwrap();
            assert!(status.durable);
            assert_eq!(status.pending_articles, 0);
            assert_eq!(status.indexed_revision, Some(status.source_revision));
        }
        std::fs::remove_dir_all(format!("{path}.search-v2")).unwrap();
        let core = BibCore::open(&path).await.unwrap();
        assert_eq!(
            core.search_articles("freshmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn paused_index_commit_does_not_block_storage_writes_or_progress_reads() {
        let (_directory, path) = database();
        let core = BibCore::open(&path).await.unwrap();
        core.upsert_article(&Article::new("a", "Initialmarker evidence"))
            .await
            .unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        core.search_index.lock().unwrap().pause_before_commit = Some((entered_tx, release_rx));
        let db = crate::actor::spawn(core);
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            db.upsert_article(&Article::new("b", "Concurrentmarker evidence")),
        )
        .await
        .expect("index commit blocked a metadata write")
        .unwrap();
        let progress = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            db.search_index_status(),
        )
        .await
        .expect("index commit blocked progress reads")
        .unwrap();
        assert!(progress.building);
        assert!(progress.pending_articles > 0);
        let page = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            db.query_collection_articles(crate::collections::CollectionQuery {
                collection_id: None,
                include_descendants: false,
                unfiled: false,
                query: String::new(),
                limit: 10,
                offset: 0,
            }),
        )
        .await
        .expect("index commit blocked ordinary library browsing")
        .unwrap();
        assert_eq!(page.total, 2);
        release_tx.send(()).unwrap();
        let hits = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            db.search_articles("concurrentmarker", 10, 0),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(hits[0].article_id, "b");
        assert_eq!(db.search_index_status().await.unwrap().pending_articles, 0);
        drop(db);
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn outbox_write_failure_rolls_back_source_changes_and_keeps_writer_usable() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("a", "Keepmarker evidence"))
            .await
            .unwrap();
        let revision = core.search_index_status().await.unwrap().source_revision;
        core.write_conn()
            .execute("DROP TABLE searchOutbox", ())
            .await
            .unwrap();
        assert!(
            core.upsert_article(&Article::new("b", "Rejectedmarker evidence"))
                .await
                .is_err()
        );
        assert!(core.delete_article("a").await.is_err());
        assert!(core.get_article("a").await.unwrap().is_some());
        assert!(core.get_article("b").await.unwrap().is_none());
        core.write_conn()
            .execute(
                "CREATE TABLE searchOutbox(articleID TEXT PRIMARY KEY, revision INTEGER NOT NULL)",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            core.search_index_status().await.unwrap().source_revision,
            revision
        );
        core.upsert_article(&Article::new("c", "Acceptedmarker evidence"))
            .await
            .unwrap();
        assert_eq!(
            core.search_articles("keepmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            core.search_articles("acceptedmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            core.search_articles("rejectedmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn equal_revision_with_a_different_commit_token_rebuilds_divergent_history() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("a", "Oldmarker evidence"))
            .await
            .unwrap();
        core.refresh_search_index().await.unwrap();
        let revision = core.search_index_status().await.unwrap().source_revision;
        core.upsert_article(&Article::new("a", "Branchmarker evidence"))
            .await
            .unwrap();
        core.write_conn()
            .execute(
                "UPDATE bib_meta SET value=?1 WHERE key='search_revision'",
                turso::params![revision.to_string()],
            )
            .await
            .unwrap();
        core.write_conn()
            .execute(
                "UPDATE searchOutbox SET revision=?1",
                turso::params![revision as i64],
            )
            .await
            .unwrap();
        assert_eq!(
            core.search_articles("branchmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            core.search_articles("oldmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_replay_rolls_back_pending_deletes_and_can_be_retried() {
        let core = BibCore::open(":memory:").await.unwrap();
        core.upsert_article(&Article::new("a", "Oldmarker evidence"))
            .await
            .unwrap();
        core.refresh_search_index().await.unwrap();
        let old_revision = core.search_index_status().await.unwrap().indexed_revision;
        core.upsert_article(&Article::new("a", "Newmarker evidence"))
            .await
            .unwrap();
        core.search_index.lock().unwrap().fail_next_commit = true;
        assert!(core.refresh_search_index().await.is_err());
        let pending = core.search_index_status().await.unwrap();
        assert_eq!(pending.indexed_revision, old_revision);
        assert_eq!(pending.pending_articles, 1);
        core.refresh_search_index().await.unwrap();
        assert_eq!(
            core.search_articles("newmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            core.search_articles("oldmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn independent_handles_replay_updates_and_deletes_without_sharing_acknowledgements() {
        let (_directory, path) = database();
        let first = BibCore::open(&path).await.unwrap();
        first
            .upsert_article(&Article::new("a", "Initialmarker evidence"))
            .await
            .unwrap();
        first.refresh_search_index().await.unwrap();
        let second = BibCore::open(&path).await.unwrap();
        assert!(!second.search_index_status().await.unwrap().durable);
        second
            .upsert_article(&Article::new("b", "Sharedmarker evidence"))
            .await
            .unwrap();
        assert_eq!(
            first
                .search_articles("sharedmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            second
                .search_articles("sharedmarker", 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        first.delete_article("b").await.unwrap();
        assert!(
            second
                .search_articles("sharedmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            first
                .search_articles("sharedmarker", 10, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
