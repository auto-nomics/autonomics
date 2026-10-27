//! On-demand, snapshot-checked evidence for one parent article.
use crate::{BibBase, Error, Result, actor::BibCommand, bib_base::BibCore};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bib_types::{SearchEvidence, SearchEvidencePage, SearchEvidenceRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use turso::transaction::{Transaction, TransactionBehavior};

#[derive(Serialize, Deserialize)]
struct Cursor {
    fingerprint: String,
    offset: usize,
}
fn invalid(message: &str) -> Error {
    Error::InvalidSearchQuery(message.into())
}
impl BibBase {
    pub async fn search_evidence(
        &self,
        article_id: &str,
        input: SearchEvidenceRequest,
    ) -> Result<SearchEvidencePage> {
        self.call(|reply| BibCommand::SearchEvidence {
            article_id: article_id.into(),
            input,
            reply,
        })
        .await
    }
}
impl BibCore {
    pub(crate) async fn search_evidence(
        &self,
        article_id: &str,
        mut input: SearchEvidenceRequest,
    ) -> Result<SearchEvidencePage> {
        if self.search_engine == crate::SearchEngineMode::Legacy {
            return Err(invalid(
                "evidence search is disabled by the legacy engine rollout flag",
            ));
        }
        input.search = crate::search::validate_request(input.search)?;
        // Presentation controls are irrelevant to evidence eligibility.
        input.search.include_articles = false;
        input.search.include_facets = false;
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = self.evidence_snapshot(article_id, input).await;
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
    async fn evidence_snapshot(
        &self,
        article_id: &str,
        mut input: SearchEvidenceRequest,
    ) -> Result<SearchEvidencePage> {
        let cursor = input.search.cursor.take();
        let (eligible, _) = self.scoped_search_candidates(&input.search).await?;
        if !eligible.iter().any(|hit| hit.article_id == article_id) {
            return Err(Error::NotFound(
                "article does not match the active search and scope".into(),
            ));
        }
        let generation = {
            let index = self
                .search_index
                .lock()
                .map_err(|_| invalid("search worker lock poisoned"))?;
            format!(
                "tantivy-v2:{}:{}",
                index.revision.unwrap_or(0),
                index.commit_token.as_deref().unwrap_or_default()
            )
        };
        if input
            .index_generation
            .as_ref()
            .is_some_and(|expected| expected != &generation)
        {
            return Err(invalid(
                "search evidence expired after a library update; refresh results",
            ));
        }
        let index = self.search_index.clone();
        let expression = input.search.expression.clone();
        let same_child = input.search.scope.require_same_child;
        let id = article_id.to_owned();
        let candidates = tokio::task::spawn_blocking(move || {
            index
                .lock()
                .map_err(|_| invalid("search worker lock poisoned"))?
                .evidence(expression.as_ref(), same_child, &id)
        })
        .await
        .map_err(|e| Error::Unknown(format!("evidence worker: {e}")))??;
        let mut digest = Sha256::new();
        digest.update(article_id.as_bytes());
        digest.update(generation.as_bytes());
        digest.update(serde_json::to_vec(&input.search)?);
        digest.update(serde_json::to_vec(&candidates)?);
        let fingerprint = format!("{:x}", digest.finalize());
        let offset = if let Some(cursor) = cursor {
            if cursor.len() > 2048 {
                return Err(invalid("invalid evidence cursor"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| invalid("invalid evidence cursor"))?;
            let value: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| invalid("invalid evidence cursor"))?;
            if value.fingerprint != fingerprint || value.offset > candidates.len() {
                return Err(invalid(
                    "evidence cursor expired or belongs to another query",
                ));
            }
            value.offset
        } else {
            0
        };
        let total = candidates.len();
        let end = offset
            .saturating_add(input.search.page_size as usize)
            .min(total);
        let next_cursor = if end < total {
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
                fingerprint,
                offset: end,
            })?))
        } else {
            None
        };
        let chosen = &candidates[offset..end];
        let rendered = self.render_search_hits(chosen, "").await?;
        let conn = self.conn();
        let mut rows=conn.query("SELECT i.key FROM articleItems m JOIN items i ON i.itemID=m.itemID WHERE m.article_id=?1",turso::params![article_id]).await?;
        let item_key = rows
            .next()
            .await?
            .ok_or_else(|| Error::NotFound(article_id.into()))?
            .get::<String>(0)?;
        drop(rows);
        let mut evidence = Vec::new();
        for (candidate, hit) in chosen.iter().zip(rendered) {
            let (mut annotation_id, mut attachment_key, mut page) = (None, None, None);
            if candidate.kind == "attachment" {
                attachment_key = Some(candidate.anchor_key.clone());
            }
            if candidate.kind == "annotation" {
                let mut rows=conn.query("SELECT n.id,n.page,p.key FROM annotations n JOIN annotationItems m ON m.annotation_id=n.id JOIN items i ON i.itemID=m.itemID LEFT JOIN itemAnnotations a ON a.itemID=m.itemID LEFT JOIN items p ON p.itemID=a.parentItemID WHERE n.article_id=?1 AND i.key=?2",turso::params![article_id,candidate.anchor_key.as_str()]).await?;
                if let Some(row) = rows.next().await? {
                    annotation_id = Some(row.get::<String>(0)?);
                    page = row
                        .get::<i64>(1)
                        .ok()
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0);
                    attachment_key = row.get::<String>(2).ok();
                }
            }
            let annotation_key =
                (candidate.kind == "annotation").then(|| candidate.anchor_key.clone());
            let location_status = if annotation_key.is_some() {
                "annotation"
            } else if attachment_key.is_some() {
                "text_search"
            } else {
                "metadata"
            };
            let attachment_hash = if let Some(key) = attachment_key.as_deref() {
                crate::zotero_model::attachment_by_key(&conn, article_id, key)
                    .await?
                    .and_then(|a| a.storage_hash)
            } else {
                None
            };
            evidence.push(SearchEvidence {
                article_id: article_id.into(),
                item_key: item_key.clone(),
                attachment_key,
                attachment_hash,
                annotation_key,
                annotation_id,
                matched_field: candidate.matched_field.clone(),
                snippet: hit.snippet,
                highlight_ranges: hit.highlight_ranges,
                matched_query: candidate.snippet_query.clone(),
                page,
                content_revision: generation.clone(),
                location_status: location_status.into(),
            });
        }
        Ok(SearchEvidencePage {
            evidence,
            total,
            next_cursor,
            index_generation: generation,
        })
    }
}
