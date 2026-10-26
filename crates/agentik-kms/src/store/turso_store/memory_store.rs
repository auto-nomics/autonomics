//! Persistent memory repository operations for [`TursoKmsStorage`].

use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{
    Error, Result, TursoKmsStorage, db_err, parse_uuid, row_to_evidence, row_to_note,
    row_to_observation, row_to_stage1, row_to_summary, text, uuid_value,
};
use crate::artifacts::KnowledgeEvidenceInput;
use crate::knowledge::Knowledge;
use crate::store::{
    KnowledgeEvidence, MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary,
    SemanticObservation,
};

#[async_trait]
impl MemoryStore for TursoKmsStorage {
    async fn get_stage1_output(&self, session_id: Uuid) -> Result<Option<MemoryStage1Record>> {
        let mut rows = self
            .conn
            .query(
                "SELECT session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                        status, generated_at, lease_until
                 FROM memory_stage1 WHERE session_id = ?1",
                params_from_iter([uuid_value(session_id)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_stage1(&row)?)),
            None => Ok(None),
        }
    }

    async fn claim_stage1(
        &self,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        let changed = self
            .conn
            .execute(
                "INSERT INTO memory_stage1
                    (session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                     status, generated_at, lease_until, attempts, updated_at)
                 VALUES (?1, ?2, '', '', NULL, 'running', 0, ?3, 1, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    status = 'running',
                    lease_until = excluded.lease_until,
                    attempts = memory_stage1.attempts + 1,
                    updated_at = excluded.updated_at
                 WHERE memory_stage1.source_hash <> excluded.source_hash
                    OR memory_stage1.status = 'failed'
                    OR (memory_stage1.status = 'running'
                        AND memory_stage1.lease_until <= excluded.updated_at)",
                params_from_iter([
                    uuid_value(session_id),
                    Value::Text(source_hash.into()),
                    Value::Integer(lease_until),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    async fn complete_stage1(&self, output: MemoryStage1Record) -> Result<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_stage1
                    (session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                     status, generated_at, lease_until, attempts, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'succeeded', ?6, 0, 1, ?7)
                 ON CONFLICT(session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    raw_memory = excluded.raw_memory,
                    rollout_summary = excluded.rollout_summary,
                    rollout_slug = excluded.rollout_slug,
                    status = 'succeeded',
                    generated_at = excluded.generated_at,
                    lease_until = 0,
                    attempts = memory_stage1.attempts + 1,
                    last_error = NULL,
                    updated_at = excluded.updated_at",
                params_from_iter([
                    uuid_value(output.session_id),
                    Value::Text(output.source_hash),
                    Value::Text(output.raw_memory),
                    Value::Text(output.rollout_summary),
                    output.rollout_slug.map(Value::Text).unwrap_or(Value::Null),
                    Value::Integer(output.generated_at),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn fail_stage1(&self, session_id: Uuid, source_hash: &str, error: &str) -> Result<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_stage1
                    (session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                     status, generated_at, lease_until, attempts, last_error, updated_at)
                 VALUES (?1, ?2, '', '', NULL, 'failed', 0, 0, 1, ?3, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    source_hash = excluded.source_hash,
                    status = 'failed',
                    lease_until = 0,
                    attempts = memory_stage1.attempts + 1,
                    last_error = excluded.last_error,
                    updated_at = excluded.updated_at",
                params_from_iter([
                    uuid_value(session_id),
                    Value::Text(source_hash.into()),
                    Value::Text(error.into()),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_stage1_outputs(&self, limit: usize) -> Result<Vec<MemoryStage1Record>> {
        self.collect(
            "SELECT session_id, source_hash, raw_memory, rollout_summary, rollout_slug,
                    status, generated_at, lease_until
             FROM memory_stage1
             WHERE status = 'succeeded'
             ORDER BY generated_at DESC, session_id DESC LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_stage1,
        )
        .await
    }

    async fn claim_phase2(&self, source_hash: &str, lease_until: i64) -> Result<bool> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        let changed = self
            .conn
            .execute(
                "INSERT INTO memory_jobs
                    (singleton, status, source_hash, lease_until, attempts, updated_at)
                 VALUES (0, 'running', ?1, ?2, 1, ?3)
                 ON CONFLICT(singleton) DO UPDATE SET
                    status = 'running',
                    source_hash = excluded.source_hash,
                    lease_until = excluded.lease_until,
                    attempts = memory_jobs.attempts + 1,
                    last_error = NULL,
                    updated_at = excluded.updated_at
                 WHERE NOT (memory_jobs.status = 'succeeded'
                        AND memory_jobs.source_hash = excluded.source_hash)
                    AND (memory_jobs.status <> 'running'
                        OR memory_jobs.lease_until <= excluded.updated_at)",
                params_from_iter([
                    Value::Text(source_hash.into()),
                    Value::Integer(lease_until),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    async fn complete_phase2(
        &self,
        source_hash: &str,
        nodes: Vec<Knowledge>,
        summary_md: &str,
        observations: Vec<SemanticObservation>,
        consumed_note_ids: Vec<Uuid>,
    ) -> Result<()> {
        let now = crate::now_ms();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            self.conn
                .execute(
                    "UPDATE knowledge_nodes SET status = 'superseded', updated_at = ?1
                     WHERE status = 'active'",
                    params_from_iter([Value::Integer(now)]),
                )
                .await
                .map_err(db_err)?;

            for node in nodes {
                self.ensure_knowledge_node(&node).await?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_summaries
                        (singleton, schema_version, summary_md, source_hash, generated_at)
                     VALUES (0, 'v1', ?1, ?2, ?3)
                     ON CONFLICT(singleton) DO UPDATE SET
                        schema_version = 'v1',
                        summary_md = excluded.summary_md,
                        source_hash = excluded.source_hash,
                        generated_at = excluded.generated_at",
                    params_from_iter([
                        Value::Text(summary_md.into()),
                        Value::Text(source_hash.into()),
                        Value::Integer(now),
                    ]),
                )
                .await
                .map_err(db_err)?;

            for observation in observations {
                let mut existing = self
                    .conn
                    .query(
                        "SELECT id FROM memory_semantic_observations
                         WHERE subject = ?1 AND predicate = ?2 AND object = ?3",
                        params_from_iter([
                            Value::Text(observation.subject.clone()),
                            Value::Text(observation.predicate.clone()),
                            Value::Text(observation.object.clone()),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
                let existing_id = match existing.next().await.map_err(db_err)? {
                    Some(row) => parse_uuid(text(&row, 0)?, "observation_id")?,
                    None => Uuid::nil(),
                };
                if existing_id != Uuid::nil() {
                    self.conn
                        .execute(
                            "UPDATE memory_semantic_observations SET
                                content = ?1, status = 'candidate', confidence = ?2,
                                source_hash = ?3, created_at = ?4, last_error = NULL
                             WHERE id = ?5",
                            params_from_iter([
                                Value::Text(observation.content),
                                Value::Real(observation.confidence),
                                Value::Text(observation.source_hash),
                                Value::Integer(observation.created_at),
                                uuid_value(existing_id),
                            ]),
                        )
                        .await
                        .map_err(db_err)?;
                    continue;
                }
                self.conn
                    .execute(
                        "INSERT INTO memory_semantic_observations
                            (id, subject, predicate, object, content, status, confidence,
                             source_hash, created_at, last_error)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'candidate', ?6, ?7, ?8, NULL)",
                        params_from_iter([
                            uuid_value(observation.id),
                            Value::Text(observation.subject),
                            Value::Text(observation.predicate),
                            Value::Text(observation.object),
                            Value::Text(observation.content),
                            Value::Real(observation.confidence),
                            Value::Text(observation.source_hash),
                            Value::Integer(now),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            }

            for note_id in consumed_note_ids {
                self.conn
                    .execute(
                        "UPDATE memory_notes SET status = 'consumed' WHERE id = ?1",
                        params_from_iter([uuid_value(note_id)]),
                    )
                    .await
                    .map_err(db_err)?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_jobs
                        (singleton, status, source_hash, lease_until, attempts, updated_at)
                     VALUES (0, 'succeeded', ?1, 0, 1, ?2)
                     ON CONFLICT(singleton) DO UPDATE SET
                        status = 'succeeded', source_hash = excluded.source_hash,
                        lease_until = 0, last_error = NULL, updated_at = excluded.updated_at",
                    params_from_iter([Value::Text(source_hash.into()), Value::Integer(now)]),
                )
                .await
                .map_err(db_err)?;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn fail_phase2(&self, source_hash: &str, error: &str) -> Result<()> {
        let updated_at = chrono::Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO memory_jobs
                    (singleton, status, source_hash, lease_until, attempts, last_error, updated_at)
                 VALUES (0, 'failed', ?1, 0, 1, ?2, ?3)
                 ON CONFLICT(singleton) DO UPDATE SET
                    status = 'failed', source_hash = excluded.source_hash,
                    lease_until = 0, attempts = memory_jobs.attempts + 1,
                    last_error = excluded.last_error, updated_at = excluded.updated_at",
                params_from_iter([
                    Value::Text(source_hash.into()),
                    Value::Text(error.into()),
                    Value::Integer(updated_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn get_summary(&self) -> Result<Option<MemorySummary>> {
        let mut rows = self
            .conn
            .query(
                "SELECT schema_version, summary_md, source_hash, generated_at
                 FROM memory_summaries WHERE singleton = 0",
                (),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_summary(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_pending_notes(&self) -> Result<Vec<MemoryNote>> {
        self.collect(
            "SELECT id, slug, content, status, created_at
             FROM memory_notes WHERE status = 'pending'
             ORDER BY created_at ASC, id ASC",
            vec![],
            row_to_note,
        )
        .await
    }

    async fn insert_note(&self, slug: &str, content: &str) -> Result<MemoryNote> {
        let note = MemoryNote {
            id: Uuid::new_v4(),
            slug: slug.into(),
            content: content.into(),
            status: "pending".into(),
            created_at: crate::now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO memory_notes
                    (id, slug, content, status, created_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4)",
                params_from_iter([
                    uuid_value(note.id),
                    Value::Text(note.slug.clone()),
                    Value::Text(note.content.clone()),
                    Value::Integer(note.created_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(note)
    }

    async fn upsert_evidence(&self, input: KnowledgeEvidenceInput) -> Result<KnowledgeEvidence> {
        let now = crate::now_ms();
        let content_hash = input.content_hash();
        let id = content_hash.clone();
        let summary = input.summary.trim().to_string();
        let body = input.body.trim().to_string();
        if summary.is_empty() || body.is_empty() {
            return Err(Error::Storage(
                "evidence summary and body must not be empty".into(),
            ));
        }
        let evidence = crate::store::KnowledgeEvidence {
            id: id.clone(),
            source_kind: input.source_kind.trim().to_string(),
            source_id: input.source_id.trim().to_string(),
            evidence_kind: input.evidence_kind.trim().to_lowercase(),
            summary,
            body,
            anchor_path: input
                .anchor_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            error: input
                .error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            content_hash,
            status: "pending".to_string(),
            occurrence_count: 1,
            created_at: now,
            last_seen_at: now,
        };
        self.conn
            .execute(
                "INSERT INTO knowledge_evidence
                    (id, source_kind, source_id, evidence_kind, summary, body,
                     anchor_path, error, content_hash, status, occurrence_count,
                     created_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', 1, ?10, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    source_kind = excluded.source_kind,
                    source_id = excluded.source_id,
                    evidence_kind = excluded.evidence_kind,
                    summary = excluded.summary,
                    body = excluded.body,
                    anchor_path = excluded.anchor_path,
                    error = excluded.error,
                    status = 'pending',
                    occurrence_count = knowledge_evidence.occurrence_count + 1,
                    last_seen_at = excluded.last_seen_at",
                params_from_iter([
                    Value::Text(evidence.id.clone()),
                    Value::Text(evidence.source_kind.clone()),
                    Value::Text(evidence.source_id.clone()),
                    Value::Text(evidence.evidence_kind.clone()),
                    Value::Text(evidence.summary.clone()),
                    Value::Text(evidence.body.clone()),
                    evidence
                        .anchor_path
                        .clone()
                        .map(Value::Text)
                        .unwrap_or(Value::Null),
                    evidence
                        .error
                        .clone()
                        .map(Value::Text)
                        .unwrap_or(Value::Null),
                    Value::Text(evidence.content_hash.clone()),
                    Value::Integer(now),
                ]),
            )
            .await
            .map_err(db_err)?;
        let mut rows = self
            .conn
            .query(
                "SELECT id, source_kind, source_id, evidence_kind, summary, body,
                        anchor_path, error, content_hash, status, occurrence_count,
                        created_at, last_seen_at
                 FROM knowledge_evidence WHERE id = ?1",
                params_from_iter([Value::Text(id)]),
            )
            .await
            .map_err(db_err)?;
        Ok(match rows.next().await.map_err(db_err)? {
            Some(row) => row_to_evidence(&row)?,
            None => evidence,
        })
    }

    async fn list_pending_evidence(&self, limit: usize) -> Result<Vec<KnowledgeEvidence>> {
        self.collect(
            "SELECT id, source_kind, source_id, evidence_kind, summary, body,
                    anchor_path, error, content_hash, status, occurrence_count,
                    created_at, last_seen_at
             FROM knowledge_evidence
             WHERE status = 'pending'
             ORDER BY last_seen_at DESC, id ASC LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_evidence,
        )
        .await
    }

    async fn list_knowledge_evidence(&self, limit: usize) -> Result<Vec<KnowledgeEvidence>> {
        self.collect(
            "SELECT id, source_kind, source_id, evidence_kind, summary, body,
                    anchor_path, error, content_hash, status, occurrence_count,
                    created_at, last_seen_at
             FROM knowledge_evidence
             ORDER BY last_seen_at DESC, id ASC LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_evidence,
        )
        .await
    }

    async fn mark_evidence_consumed(&self, evidence_ids: Vec<String>) -> Result<()> {
        if evidence_ids.is_empty() {
            return Ok(());
        }
        let now = crate::now_ms();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            for id in evidence_ids {
                self.conn
                    .execute(
                        "UPDATE knowledge_evidence
                         SET status = 'consumed', last_seen_at = ?1
                         WHERE id = ?2 AND status = 'pending'",
                        params_from_iter([Value::Integer(now), Value::Text(id)]),
                    )
                    .await
                    .map_err(db_err)?;
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn has_evidence_source(&self, source_kind: &str, source_id: &str) -> Result<bool> {
        let mut rows = self
            .conn
            .query(
                "SELECT 1 FROM knowledge_evidence
                 WHERE source_kind = ?1 AND source_id = ?2 LIMIT 1",
                params_from_iter([
                    Value::Text(source_kind.to_string()),
                    Value::Text(source_id.to_string()),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(rows.next().await.map_err(db_err)?.is_some())
    }

    async fn import_skill_version(
        &self,
        input: crate::artifacts::KnowledgeSkillVersionInput,
    ) -> Result<Option<crate::store::SkillVersion>> {
        self.import_skill_version_inner(input).await
    }

    async fn list_skill_versions(&self, limit: usize) -> Result<Vec<crate::store::SkillVersion>> {
        self.list_skill_versions_inner(limit).await
    }

    async fn list_skill_artifacts(
        &self,
        skill_version_id: Uuid,
    ) -> Result<Vec<crate::store::SkillArtifact>> {
        self.list_skill_artifacts_inner(skill_version_id).await
    }

    async fn create_skill_proposal(
        &self,
        input: crate::artifacts::KnowledgeSkillProposalInput,
    ) -> Result<Option<crate::store::SkillProposal>> {
        self.create_skill_proposal_inner(input).await
    }

    async fn list_skill_proposals(&self, limit: usize) -> Result<Vec<crate::store::SkillProposal>> {
        self.list_skill_proposals_inner(limit).await
    }

    async fn approve_skill_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<Option<crate::store::SkillVersion>> {
        self.approve_skill_proposal_inner(proposal_id).await
    }

    async fn reject_skill_proposal(&self, proposal_id: Uuid) -> Result<bool> {
        self.reject_skill_proposal_inner(proposal_id).await
    }

    async fn list_observations(
        &self,
        status: &str,
        limit: usize,
    ) -> Result<Vec<SemanticObservation>> {
        self.collect(
            "SELECT id, subject, predicate, object, content, status, confidence,
                    source_hash, created_at, last_error
             FROM memory_semantic_observations
             WHERE status = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2",
            vec![Value::Text(status.into()), Value::Integer(limit as i64)],
            row_to_observation,
        )
        .await
    }

    async fn set_observation_status(
        &self,
        observation_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE memory_semantic_observations
                 SET status = ?1, last_error = ?2 WHERE id = ?3",
                params_from_iter([
                    Value::Text(status.into()),
                    error
                        .map(|value| Value::Text(value.into()))
                        .unwrap_or(Value::Null),
                    uuid_value(observation_id),
                ]),
            )
            .await
            .map_err(db_err)?;
        if changed == 0 {
            return Err(Error::Storage(format!(
                "semantic observation {observation_id} not found"
            )));
        }
        Ok(())
    }
}
