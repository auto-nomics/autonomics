//! Persisted candidate scenes, accepted atomically against their observed revision.
use crate::{Error, Result, Store, Visualization};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
pub(crate) fn document_hash(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub visualization_id: String,
    pub base_revision: i64,
    pub summary: String,
    pub document: Value,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub document_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_proposal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_document_sha256: Option<String>,
    #[serde(default)]
    pub review_markups: Vec<crate::markup::Summary>,
}
#[derive(Debug, Serialize)]
pub struct PendingSummary {
    pub current_revision: i64,
    pub total: i64,
    pub current: i64,
    pub stale: i64,
    pub latest: Option<PendingCandidate>,
}
#[derive(Debug, Serialize)]
pub struct PendingCandidate {
    pub id: String,
    pub summary: String,
    pub summary_truncated: bool,
    pub base_revision: i64,
    pub parent_proposal_id: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct ProposalIndex {
    pub items: Vec<PendingCandidate>,
    pub next_before: Option<String>,
}
pub struct ProposedRevision<'a> {
    pub parent_id: &'a str,
    pub parent_document_sha256: &'a str,
    pub summary: &'a str,
    pub document: &'a Value,
    pub markup_ids: &'a [String],
}
struct Draft<'a> {
    project: &'a str,
    id: &'a str,
    base_revision: i64,
    summary: &'a str,
    document: &'a Value,
    markup_ids: &'a [String],
    parent: Option<(&'a str, &'a str)>,
}
impl Store {
    pub fn propose(
        &self,
        project: &str,
        id: &str,
        base_revision: i64,
        summary: &str,
        document: &Value,
    ) -> Result<Proposal> {
        self.propose_with_markups(project, id, base_revision, summary, document, &[])
    }
    /// Attach only explicitly addressed open reviews. Links do not resolve them.
    pub fn propose_with_markups(
        &self,
        project: &str,
        id: &str,
        base_revision: i64,
        summary: &str,
        document: &Value,
        markup_ids: &[String],
    ) -> Result<Proposal> {
        self.insert_proposal(Draft {
            project,
            id,
            base_revision,
            summary,
            document,
            markup_ids,
            parent: None,
        })
    }
    pub fn propose_revision(
        &self,
        project: &str,
        id: &str,
        base_revision: i64,
        revision: ProposedRevision<'_>,
    ) -> Result<Proposal> {
        self.insert_proposal(Draft {
            project,
            id,
            base_revision,
            summary: revision.summary,
            document: revision.document,
            markup_ids: revision.markup_ids,
            parent: Some((revision.parent_id, revision.parent_document_sha256)),
        })
    }
    fn insert_proposal(&self, draft: Draft<'_>) -> Result<Proposal> {
        let Draft {
            project,
            id,
            base_revision,
            summary,
            document,
            markup_ids,
            parent,
        } = draft;
        if markup_ids.len() > 50
            || markup_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != markup_ids.len()
        {
            return Err(Error::Invalid("at most 50 distinct review markup IDs"));
        }
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if summary.trim().is_empty() {
            return Err(Error::Invalid("proposal summary"));
        }
        if self.get(project, id)?.revision != base_revision {
            return Err(Error::Conflict);
        }
        if let Some((parent_id, expected_hash)) = parent {
            let row=transaction.query_row("SELECT base_revision,status,document FROM visualization_proposals WHERE id=?1 AND visualization_id=?2",params![parent_id,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?.ok_or(Error::NotFound)?;
            if row.0 != base_revision
                || row.1 != "pending"
                || document_hash(&row.2) != expected_hash
            {
                return Err(Error::Conflict);
            }
        }
        if self.linear_candidate_revisions {
            if let Some((parent_id, _)) = parent {
                let has_child: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM visualization_proposal_lineage l JOIN visualization_proposals p ON p.id=l.proposal_id WHERE l.parent_id=?1 AND p.status='pending')",
                    params![parent_id], |row| row.get(0),
                )?;
                if has_child {
                    return Err(Error::Invalid(
                        "revision parent already has a pending child; use list_proposals and read_proposal to continue the intended leaf with its document_sha256; nothing was saved",
                    ));
                }
            }
        }
        let snapshot = serde_json::to_string(document)?;
        let mut proposal = Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            visualization_id: id.into(),
            base_revision,
            summary: summary.trim().into(),
            document: document.clone(),
            status: "pending".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            document_sha256: document_hash(&snapshot),
            parent_proposal_id: parent.map(|p| p.0.to_owned()),
            parent_document_sha256: parent.map(|p| p.1.to_owned()),
            review_markups: Vec::new(),
        };
        let inserted = self.connection.execute(
            "INSERT INTO visualization_proposals SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE EXISTS (SELECT 1 FROM visualizations WHERE id=?2 AND project_id=?8 AND revision=?3)",
            params![
                proposal.id,
                id,
                base_revision,
                proposal.summary,
                snapshot,
                proposal.status,
                proposal.created_at,
                project
            ],
        )?;
        if inserted == 0 {
            return Err(Error::Conflict);
        }
        for markup_id in markup_ids {
            let status: Option<String> = transaction
                .query_row(
                    "SELECT status FROM visualization_markups WHERE visualization_id=?1 AND id=?2",
                    params![id, markup_id],
                    |row| row.get(0),
                )
                .optional()?;
            if status.ok_or(Error::Invalid(
                "markup_ids: no matching sketch markup in this figure; object annotation IDs are a separate channel and cannot be linked here; use IDs from list_markups/read_markup only, or omit markup_ids when no sketch markups are addressed; nothing was saved",
            ))? != "open" {
                return Err(Error::Conflict);
            }
            transaction.execute(
                "INSERT INTO visualization_proposal_markups VALUES (?1,?2)",
                params![proposal.id, markup_id],
            )?;
        }
        if let Some((parent_id, expected_hash)) = parent {
            transaction.execute(
                "INSERT INTO visualization_proposal_lineage VALUES(?1,?2,?3)",
                params![proposal.id, parent_id, expected_hash],
            )?;
        }
        proposal.review_markups = self.proposal_markups(&proposal.id)?;
        transaction.commit()?;
        Ok(proposal)
    }
    fn proposal_markups(&self, proposal_id: &str) -> Result<Vec<crate::markup::Summary>> {
        let mut statement = self.connection.prepare("SELECT m.id,m.visualization_id,m.at_revision,m.note,m.strokes,m.status,m.created_at FROM visualization_markups m JOIN visualization_proposal_markups p ON p.markup_id=m.id WHERE p.proposal_id=?1 ORDER BY m.created_at,m.id")?;
        let rows = statement.query_map([proposal_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (id, visualization_id, at_revision, note, strokes, status, created_at) = row?;
            Ok(crate::markup::Summary {
                id,
                visualization_id,
                at_revision,
                note,
                stroke_count: serde_json::from_str::<Vec<crate::markup::Stroke>>(&strokes)?.len(),
                status,
                created_at,
            })
        })
        .collect()
    }
    pub fn proposal(&self, project: &str, id: &str, proposal_id: &str) -> Result<Proposal> {
        self.get(project, id)?;
        let row=self.connection.query_row("SELECT p.base_revision,p.summary,p.document,p.status,p.created_at,l.parent_id,l.parent_document_sha256 FROM visualization_proposals p LEFT JOIN visualization_proposal_lineage l ON l.proposal_id=p.id WHERE p.id=?1 AND p.visualization_id=?2",params![proposal_id,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?))).optional()?.ok_or(Error::NotFound)?;
        Ok(Proposal {
            id: proposal_id.into(),
            visualization_id: id.into(),
            base_revision: row.0,
            summary: row.1,
            document_sha256: document_hash(&row.2),
            document: serde_json::from_str(&row.2)?,
            status: row.3,
            created_at: row.4,
            parent_proposal_id: row.5,
            parent_document_sha256: row.6,
            review_markups: self.proposal_markups(proposal_id)?,
        })
    }
    /// Page candidate metadata without reading scene bytes.
    pub fn proposal_index(
        &self,
        project: &str,
        id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<ProposalIndex> {
        self.pending_candidate_index(project, id, None, before, limit)
    }
    /// Pending direct children, scoped to one parent without decoding scenes.
    pub fn proposal_children_index(
        &self,
        project: &str,
        id: &str,
        parent: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<ProposalIndex> {
        self.pending_candidate_index(project, id, Some(parent), before, limit)
    }
    fn pending_candidate_index(
        &self,
        project: &str,
        id: &str,
        parent: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> Result<ProposalIndex> {
        if !(1..=100).contains(&limit) {
            return Err(Error::Invalid("proposal page limit must be 1..100"));
        }
        self.get(project, id)?;
        if let Some(parent) = parent {
            self.connection
                .query_row(
                    "SELECT 1 FROM visualization_proposals WHERE id=?1 AND visualization_id=?2",
                    params![parent, id],
                    |_| Ok(()),
                )
                .optional()?
                .ok_or(Error::NotFound)?;
        }
        let cursor: Option<(String,i64)> = before.map(|cursor| self.connection.query_row(
            "SELECT p.created_at,p.rowid FROM visualization_proposals p LEFT JOIN visualization_proposal_lineage l ON l.proposal_id=p.id WHERE p.id=?1 AND p.visualization_id=?2 AND (?3 IS NULL OR l.parent_id=?3)", params![cursor,id,parent], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(Error::NotFound)).transpose()?;
        let mut statement = self.connection.prepare(
            "SELECT p.id,substr(p.summary,1,1000),length(p.summary)>1000,p.base_revision,l.parent_id FROM visualization_proposals p LEFT JOIN visualization_proposal_lineage l ON l.proposal_id=p.id WHERE p.visualization_id=?1 AND p.status='pending' AND (?5 IS NULL OR l.parent_id=?5) AND (?2 IS NULL OR p.created_at<?2 OR (p.created_at=?2 AND p.rowid<?3)) ORDER BY p.created_at DESC,p.rowid DESC LIMIT ?4")?;
        let mut items = statement
            .query_map(
                params![
                    id,
                    cursor.as_ref().map(|v| &v.0),
                    cursor.as_ref().map(|v| v.1),
                    limit as i64 + 1,
                    parent
                ],
                |r| {
                    Ok(PendingCandidate {
                        id: r.get(0)?,
                        summary: r.get(1)?,
                        summary_truncated: r.get(2)?,
                        base_revision: r.get(3)?,
                        parent_proposal_id: r.get(4)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next_before = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        Ok(ProposalIndex { items, next_before })
    }
    pub fn pending_proposal_summary(&self, project: &str, id: &str) -> Result<PendingSummary> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = self.get(project, id)?.revision;
        let (total, current): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*),COALESCE(SUM(base_revision=?2),0) FROM visualization_proposals WHERE visualization_id=?1 AND status='pending'",
            params![id, revision], |row| Ok((row.get(0)?,row.get(1)?)))?;
        let latest = transaction.query_row(
            "SELECT p.id,substr(p.summary,1,1000),length(p.summary)>1000,p.base_revision,l.parent_id FROM visualization_proposals p LEFT JOIN visualization_proposal_lineage l ON l.proposal_id=p.id WHERE p.visualization_id=?1 AND p.status='pending' ORDER BY (p.base_revision=?2) DESC,p.created_at DESC,p.rowid DESC LIMIT 1",
            params![id, revision], |row| Ok(PendingCandidate { id: row.get(0)?, summary: row.get(1)?, summary_truncated: row.get(2)?, base_revision: row.get(3)?, parent_proposal_id: row.get(4)? })).optional()?;
        transaction.commit()?;
        Ok(PendingSummary {
            current_revision: revision,
            total,
            current,
            stale: total - current,
            latest,
        })
    }
    pub fn proposals(&self, project: &str, id: &str) -> Result<Vec<Proposal>> {
        self.get(project, id)?;
        let mut statement=self.connection.prepare("SELECT p.id,p.base_revision,p.summary,p.document,p.status,p.created_at,l.parent_id,l.parent_document_sha256 FROM visualization_proposals p LEFT JOIN visualization_proposal_lineage l ON l.proposal_id=p.id WHERE p.visualization_id=?1 ORDER BY p.created_at DESC,p.id")?;
        let rows = statement.query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (
                proposal_id,
                base_revision,
                summary,
                document,
                status,
                created_at,
                parent_proposal_id,
                parent_document_sha256,
            ) = row?;
            let review_markups = self.proposal_markups(&proposal_id)?;
            Ok(Proposal {
                id: proposal_id,
                visualization_id: id.into(),
                base_revision,
                summary,
                document_sha256: document_hash(&document),
                parent_proposal_id,
                parent_document_sha256,
                document: serde_json::from_str(&document)?,
                status,
                created_at,
                review_markups,
            })
        })
        .collect()
    }
    pub fn accept_proposal(
        &mut self,
        project: &str,
        id: &str,
        proposal_id: &str,
    ) -> Result<Visualization> {
        self.accept_proposal_with_reviews(project, id, proposal_id, false)
    }
    /// Closing review notes is an explicit user choice and shares the scene transaction.
    pub fn accept_proposal_with_reviews(
        &mut self,
        project: &str,
        id: &str,
        proposal_id: &str,
        resolve_markups: bool,
    ) -> Result<Visualization> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut metadata = transaction.query_row("SELECT id,project_id,title,format,revision,created_at,updated_at FROM visualizations WHERE project_id=?1 AND id=?2", params![project,id], crate::storage::metadata).optional()?.ok_or(Error::NotFound)?;
        let row:Option<(i64,String,String)>=transaction.query_row("SELECT base_revision,document,status FROM visualization_proposals WHERE id=?1 AND visualization_id=?2",params![proposal_id,id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        let (base_revision, document, status) = row.ok_or(Error::NotFound)?;
        if status != "pending" {
            return Err(Error::Conflict);
        }
        let timestamp = chrono::Utc::now().to_rfc3339();
        if transaction.execute("UPDATE visualizations SET revision=revision+1,updated_at=?1 WHERE id=?2 AND project_id=?3 AND revision=?4",params![timestamp,id,project,base_revision])?==0 {return Err(Error::Conflict);}
        metadata.revision = base_revision + 1;
        metadata.updated_at = timestamp;
        transaction.execute(
            "INSERT INTO visualization_revisions VALUES (?1,?2,?3,?4,?5)",
            params![
                id,
                metadata.revision,
                metadata.title,
                document,
                metadata.updated_at
            ],
        )?;
        transaction.execute(
            "UPDATE visualization_proposals SET status='accepted' WHERE id=?1",
            [proposal_id],
        )?;
        if resolve_markups {
            transaction.execute("UPDATE visualization_markups SET status='resolved' WHERE visualization_id=?1 AND id IN (SELECT markup_id FROM visualization_proposal_markups WHERE proposal_id=?2)", params![id,proposal_id])?;
        }
        transaction.commit()?;
        Ok(metadata)
    }
    pub fn reject_proposal(&self, project: &str, id: &str, proposal_id: &str) -> Result<()> {
        self.get(project, id)?;
        if self.connection.execute("UPDATE visualization_proposals SET status='rejected' WHERE id=?1 AND visualization_id=?2 AND status='pending'",params![proposal_id,id])?==0{return Err(Error::Conflict);}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn child_index_is_parent_scoped_paginated_and_never_decodes_documents() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let scene = json!({"elements":[],"files":{"asset":{"dataURL":"PRIVATE-BYTES"}}});
        let figure = store.create("p", "Figure", "excalidraw/v2", &scene)?;
        let other = store.create("p", "Other", "excalidraw/v2", &scene)?;
        let parent = store.propose("p", &figure.id, 1, "Parent", &scene)?;
        let sibling = store.propose("p", &figure.id, 1, "Other root", &scene)?;
        let mut children = Vec::new();
        for index in 0..25 {
            children.push(store.propose_revision(
                "p",
                &figure.id,
                1,
                ProposedRevision {
                    parent_id: &parent.id,
                    parent_document_sha256: &parent.document_sha256,
                    summary: &format!("Child {index}"),
                    document: &scene,
                    markup_ids: &[],
                },
            )?);
        }
        let grandchild = store.propose_revision(
            "p",
            &figure.id,
            1,
            ProposedRevision {
                parent_id: &children[0].id,
                parent_document_sha256: &children[0].document_sha256,
                summary: "Grandchild",
                document: &scene,
                markup_ids: &[],
            },
        )?;
        store.connection.execute("UPDATE visualization_proposals SET created_at='same',document='not-json' WHERE visualization_id=?1", [&figure.id])?;
        let first = store.proposal_children_index("p", &figure.id, &parent.id, None, 20)?;
        assert_eq!(first.items.len(), 20);
        assert_eq!(first.items[0].id, children[24].id);
        assert_eq!(first.items[19].id, children[5].id);
        assert!(
            first
                .items
                .iter()
                .all(|item| item.parent_proposal_id.as_deref() == Some(&parent.id))
        );
        let serialized = serde_json::to_string(&first)?;
        assert!(!serialized.contains("PRIVATE-BYTES") && !serialized.contains("document"));
        let cursor = first.next_before.unwrap();
        store.reject_proposal("p", &figure.id, &cursor)?;
        let second =
            store.proposal_children_index("p", &figure.id, &parent.id, Some(&cursor), 20)?;
        assert_eq!(
            second.items.iter().map(|v| &v.id).collect::<Vec<_>>(),
            children[..5]
                .iter()
                .rev()
                .map(|v| &v.id)
                .collect::<Vec<_>>()
        );
        assert!(second.next_before.is_none());
        assert_eq!(
            store
                .proposal_children_index("p", &figure.id, &children[0].id, None, 20)?
                .items[0]
                .id,
            grandchild.id
        );
        assert!(
            store
                .proposal_children_index("p", &figure.id, &sibling.id, None, 20)?
                .items
                .is_empty()
        );
        for (figure_id, parent_id, before) in [
            (&other.id, &parent.id, None),
            (&figure.id, &sibling.id, Some(cursor.as_str())),
            (&figure.id, &parent.id, Some(sibling.id.as_str())),
            (&figure.id, &parent.id, Some(grandchild.id.as_str())),
        ] {
            assert!(matches!(
                store.proposal_children_index("p", figure_id, parent_id, before, 20),
                Err(Error::NotFound)
            ));
        }
        for limit in [0, 101] {
            assert!(
                store
                    .proposal_children_index("p", &figure.id, &parent.id, None, limit)
                    .is_err()
            );
        }
        Ok(())
    }
    #[test]
    fn candidate_index_pages_metadata_with_stable_scoped_cursors() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let scene = json!({"elements":[],"files":{"asset":{"dataURL":"PRIVATE-BYTES"}}});
        let figure = store.create("p", "Figure", "excalidraw/v2", &scene)?;
        let other = store.create("p", "Other", "excalidraw/v2", &scene)?;
        let mut ids = Vec::new();
        for index in 0..25 {
            ids.push(
                store
                    .propose("p", &figure.id, 1, &format!("Candidate {index}"), &scene)?
                    .id,
            );
        }
        // Equal timestamps must still have a deterministic insertion order.
        store.connection.execute("UPDATE visualization_proposals SET created_at='same',document='not-json' WHERE visualization_id=?1", [&figure.id])?;
        let first = store.proposal_index("p", &figure.id, None, 20)?;
        assert_eq!(first.items.len(), 20);
        assert_eq!(first.items[0].id, ids[24]);
        assert_eq!(first.items[19].id, ids[5]);
        assert!(!serde_json::to_string(&first)?.contains("PRIVATE-BYTES"));
        let cursor = first.next_before.unwrap();
        store.reject_proposal("p", &figure.id, &cursor)?;
        let second = store.proposal_index("p", &figure.id, Some(&cursor), 20)?;
        assert_eq!(
            second.items.iter().map(|v| &v.id).collect::<Vec<_>>(),
            ids[..5].iter().rev().collect::<Vec<_>>()
        );
        assert!(second.next_before.is_none());
        assert!(matches!(
            store.proposal_index("p", &other.id, Some(&cursor), 20),
            Err(Error::NotFound)
        ));
        assert!(store.proposal_index("p", &figure.id, None, 0).is_err());
        assert!(store.proposal_index("p", &figure.id, None, 101).is_err());
        Ok(())
    }
    #[test]
    fn pending_notifications_are_scoped_bounded_and_exclude_scene_bytes() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let scene = json!({"elements":[],"files":{"asset":{"dataURL":"PRIVATE-IMAGE-BYTES"}}});
        let figure = store.create("p", "Figure", "excalidraw/v2", &scene)?;
        assert_eq!(store.pending_proposal_summary("p", &figure.id)?.total, 0);
        assert!(matches!(
            store.pending_proposal_summary("other", &figure.id),
            Err(Error::NotFound)
        ));
        let old = store.propose("p", &figure.id, 1, "Old", &scene)?;
        let adopted = store.propose("p", &figure.id, 1, "Adopt", &scene)?;
        store.accept_proposal("p", &figure.id, &adopted.id)?;
        let stale = store.pending_proposal_summary("p", &figure.id)?;
        assert_eq!(
            (
                stale.current_revision,
                stale.total,
                stale.current,
                stale.stale
            ),
            (2, 1, 0, 1)
        );
        assert_eq!(stale.latest.unwrap().id, old.id);
        let latest = store.propose("p", &figure.id, 2, &"图".repeat(1200), &scene)?;
        // The notification must not parse or transfer candidate documents.
        store.connection.execute(
            "UPDATE visualization_proposals SET document='not-json' WHERE id=?1",
            [&latest.id],
        )?;
        let current = store.pending_proposal_summary("p", &figure.id)?;
        assert_eq!((current.total, current.current, current.stale), (2, 1, 1));
        assert!(!serde_json::to_string(&current)?.contains("PRIVATE-IMAGE-BYTES"));
        let candidate = current.latest.unwrap();
        assert_eq!(candidate.id, latest.id);
        assert_eq!(candidate.summary.chars().count(), 1000);
        assert!(candidate.summary_truncated);
        store.reject_proposal("p", &figure.id, &latest.id)?;
        store.reject_proposal("p", &figure.id, &old.id)?;
        let empty = store.pending_proposal_summary("p", &figure.id)?;
        assert_eq!(empty.total, 0);
        assert!(empty.latest.is_none());
        Ok(())
    }
    #[test]
    fn linked_reviews_close_only_on_explicit_atomic_adoption() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let other = store.create("p", "Other", "excalidraw/v2", &json!({"elements":[]}))?;
        let ink = [crate::markup::Stroke {
            kind: crate::markup::Kind::Cross,
            points: vec![[0., 0.], [10., 10.]],
        }];
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let review =
            store.create_markup("p", &figure.id, 1, "Remove the crossed object", &ink, png)?;
        let unrelated = store.create_markup("p", &figure.id, 1, "Unaddressed", &ink, png)?;
        let ids = vec![review.id.clone()];
        for bad in [
            vec!["missing".into()],
            vec![review.id.clone(), review.id.clone()],
            vec![review.id.clone(); 51],
        ] {
            assert!(
                store
                    .propose_with_markups(
                        "p",
                        &figure.id,
                        1,
                        "Invalid",
                        &json!({"elements":[]}),
                        &bad
                    )
                    .is_err()
            );
        }
        let foreign = store.propose_with_markups("p", &other.id, 1, "Foreign", &json!({"elements":[]}), &ids).unwrap_err();
        assert!(matches!(foreign, Error::Invalid(_)));
        assert!(foreign.to_string().contains("no matching sketch markup in this figure"));
        assert!(store.proposals("p", &other.id)?.is_empty());
        assert!(store.proposals("p", &figure.id)?.is_empty());
        let rejected = store.propose_with_markups(
            "p",
            &figure.id,
            1,
            "Rejected",
            &json!({"elements":[]}),
            &ids,
        )?;
        assert_eq!(rejected.review_markups[0].note, review.note);
        assert!(!serde_json::to_string(&rejected)?.contains("iVBOR"));
        store.reject_proposal("p", &figure.id, &rejected.id)?;
        assert_eq!(
            store.markup("p", &figure.id, &review.id)?.summary.status,
            "open"
        );
        let plain = store.propose_with_markups(
            "p",
            &figure.id,
            1,
            "Artwork only",
            &json!({"elements":[]}),
            &ids,
        )?;
        store.accept_proposal("p", &figure.id, &plain.id)?;
        assert_eq!(
            store.markup("p", &figure.id, &review.id)?.summary.status,
            "open"
        );
        // Historical reviews may be reconciled with the current scene and linked.
        let candidate = store.propose_with_markups(
            "p",
            &figure.id,
            2,
            "Address review",
            &json!({"elements":[{"id":"new"}]}),
            &ids,
        )?;
        // A failure in review completion rolls back artwork, revision and proposal status.
        store.connection.execute_batch("CREATE TRIGGER fail_review BEFORE UPDATE OF status ON visualization_markups BEGIN SELECT RAISE(ABORT,'test failure'); END;")?;
        assert!(
            store
                .accept_proposal_with_reviews("p", &figure.id, &candidate.id, true)
                .is_err()
        );
        assert_eq!(store.get("p", &figure.id)?.revision, 2);
        assert!(matches!(
            store.revision("p", &figure.id, 3),
            Err(Error::NotFound)
        ));
        assert_eq!(
            store
                .proposals("p", &figure.id)?
                .iter()
                .find(|p| p.id == candidate.id)
                .unwrap()
                .status,
            "pending"
        );
        store
            .connection
            .execute_batch("DROP TRIGGER fail_review;")?;
        store.accept_proposal_with_reviews("p", &figure.id, &candidate.id, true)?;
        assert_eq!(store.get("p", &figure.id)?.revision, 3);
        assert_eq!(
            store.markup("p", &figure.id, &review.id)?.summary.status,
            "resolved"
        );
        assert_eq!(
            store.markup("p", &figure.id, &unrelated.id)?.summary.status,
            "open"
        );
        assert!(matches!(
            store.propose_with_markups("p", &figure.id, 3, "Closed", &json!({"elements":[]}), &ids),
            Err(Error::Conflict)
        ));
        store.delete("p", &figure.id, 3)?;
        assert_eq!(
            store.connection.query_row(
                "SELECT count(*) FROM visualization_proposal_markups",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
    #[test]
    fn stale_review_proposals_leave_notes_open() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let ink = [crate::markup::Stroke {
            kind: crate::markup::Kind::Cross,
            points: vec![[0., 0.], [10., 10.]],
        }];
        let review = store.create_markup("p", &figure.id, 1, "Review", &ink, png)?;
        let candidate = store.propose_with_markups(
            "p",
            &figure.id,
            1,
            "Candidate",
            &json!({"elements":[]}),
            std::slice::from_ref(&review.id),
        )?;
        store.save(
            "p",
            &figure.id,
            1,
            "Manual edit",
            &json!({"elements":[{"id":"keep"}]}),
        )?;
        assert!(matches!(
            store.accept_proposal_with_reviews("p", &figure.id, &candidate.id, true),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store.markup("p", &figure.id, &review.id)?.summary.status,
            "open"
        );
        assert_eq!(
            store.revision("p", &figure.id, 2)?.document["elements"][0]["id"],
            "keep"
        );
        Ok(())
    }
    #[test]
    fn independent_writers_accept_only_one_candidate_per_revision() -> Result<()> {
        use std::sync::{Arc, Barrier};
        let path = std::env::temp_dir().join(format!(
            "visualization-race-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let mut observer = Store::open(&path)?;
        observer.create_project("p", "Study")?;
        let mut a = Vec::new();
        let mut b = Vec::new();
        for index in 0..20 {
            let figure = observer.create(
                "p",
                &format!("Figure {index}"),
                "excalidraw/v2",
                &json!({"elements":[]}),
            )?;
            let left =
                observer.propose("p", &figure.id, 1, "A", &json!({"elements":[{"id":"a"}]}))?;
            let right =
                observer.propose("p", &figure.id, 1, "B", &json!({"elements":[{"id":"b"}]}))?;
            a.push((figure.id.clone(), left.id));
            b.push((figure.id, right.id));
        }
        let left = Store::open(&path)?;
        let right = Store::open(&path)?;
        let barrier = Arc::new(Barrier::new(2));
        let run = |mut store: Store, inputs: Vec<(String, String)>, barrier: Arc<Barrier>| {
            std::thread::spawn(move || {
                inputs
                    .into_iter()
                    .map(|(figure, proposal)| {
                        barrier.wait();
                        store.accept_proposal("p", &figure, &proposal)
                    })
                    .collect::<Vec<_>>()
            })
        };
        let a = run(left, a, barrier.clone());
        let b = run(right, b, barrier);
        for (a, b) in a.join().unwrap().into_iter().zip(b.join().unwrap()) {
            match (a, b) {
                (Ok(metadata), Err(Error::Conflict)) | (Err(Error::Conflict), Ok(metadata)) => {
                    assert_eq!(metadata.revision, 2);
                    assert_eq!(
                        observer.revision("p", &metadata.id, 2)?.title,
                        observer.get("p", &metadata.id)?.title
                    );
                }
                results => panic!("one winner and one revision conflict required, got {results:?}"),
            }
        }
        drop(observer);
        std::fs::remove_file(path).unwrap();
        Ok(())
    }
    #[test]
    fn proposals_do_not_modify_scene_until_accepted_and_detect_stale_writes() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let proposal = store.propose(
            "p",
            &figure.id,
            1,
            "Add cell",
            &json!({"elements":[{"id":"cell"}]}),
        )?;
        assert_eq!(store.get("p", &figure.id)?.revision, 1);
        assert!(matches!(
            store.accept_proposal("other", &figure.id, &proposal.id),
            Err(Error::NotFound)
        ));
        assert_eq!(
            store
                .accept_proposal("p", &figure.id, &proposal.id)?
                .revision,
            2
        );
        assert!(matches!(
            store.accept_proposal("p", &figure.id, &proposal.id),
            Err(Error::Conflict)
        ));
        let stale = store.propose(
            "p",
            &figure.id,
            2,
            "Stale candidate",
            &json!({"elements":[]}),
        )?;
        store.save(
            "p",
            &figure.id,
            2,
            "Manual edit",
            &json!({"elements":[{"id":"manual"}]}),
        )?;
        assert!(matches!(
            store.accept_proposal("p", &figure.id, &stale.id),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store.revision("p", &figure.id, 3)?.document["elements"][0]["id"],
            "manual"
        );
        store.reject_proposal("p", &figure.id, &stale.id)?;
        assert_eq!(store.proposals("p", &figure.id)?.len(), 2);
        Ok(())
    }
}
