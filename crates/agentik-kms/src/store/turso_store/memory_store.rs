//! Persistent memory repository operations for [`TursoKmsStorage`].

use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{
    Error, Result, TursoKmsStorage, db_err, parse_uuid, row_to_note, row_to_observation,
    row_to_stage1, row_to_summary, text, uuid_value,
};
use crate::knowledge::Knowledge;
use crate::store::{
    MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary, SemanticObservation,
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
