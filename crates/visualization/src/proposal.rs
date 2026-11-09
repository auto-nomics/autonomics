//! Persisted candidate scenes, accepted atomically against their observed revision.
use crate::{Error, Result, Store, Visualization};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub visualization_id: String,
    pub base_revision: i64,
    pub summary: String,
    pub document: Value,
    pub status: String,
    pub created_at: String,
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
        if summary.trim().is_empty() {
            return Err(Error::Invalid("proposal summary"));
        }
        if self.get(project, id)?.revision != base_revision {
            return Err(Error::Conflict);
        }
        let proposal = Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            visualization_id: id.into(),
            base_revision,
            summary: summary.trim().into(),
            document: document.clone(),
            status: "pending".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let inserted = self.connection.execute(
            "INSERT INTO visualization_proposals SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE EXISTS (SELECT 1 FROM visualizations WHERE id=?2 AND project_id=?8 AND revision=?3)",
            params![
                proposal.id,
                id,
                base_revision,
                proposal.summary,
                serde_json::to_string(document)?,
                proposal.status,
                proposal.created_at,
                project
            ],
        )?;
        if inserted == 0 {
            return Err(Error::Conflict);
        }
        Ok(proposal)
    }
    pub fn proposals(&self, project: &str, id: &str) -> Result<Vec<Proposal>> {
        self.get(project, id)?;
        let mut statement=self.connection.prepare("SELECT id,base_revision,summary,document,status,created_at FROM visualization_proposals WHERE visualization_id=?1 ORDER BY created_at DESC,id")?;
        let rows = statement.query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (proposal_id, base_revision, summary, document, status, created_at) = row?;
            Ok(Proposal {
                id: proposal_id,
                visualization_id: id.into(),
                base_revision,
                summary,
                document: serde_json::from_str(&document)?,
                status,
                created_at,
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
