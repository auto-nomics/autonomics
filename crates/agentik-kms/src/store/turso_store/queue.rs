use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{Error, Result, TursoKmsStorage, db_err, int, text, uuid_value};
use crate::now_ms;
use crate::pipeline::{KnowledgeJob, StageOutcome, StageStatus};
use crate::store::{KnowledgeQueue, QueuedKnowledgeJob};

const DEFAULT_MAX_ATTEMPTS: i64 = 5;
const LEASE_MS: i64 = 3_600_000;

#[async_trait]
impl KnowledgeQueue for TursoKmsStorage {
    async fn enqueue(&self, job: KnowledgeJob) -> Result<Uuid> {
        let now = now_ms();
        let dedup_key = job.dedup_key();
        let mut existing = self
            .conn
            .query(
                "SELECT id, status, lease_until FROM knowledge_pipeline_jobs
                 WHERE dedup_key = ?1",
                params_from_iter([Value::Text(dedup_key.clone())]),
            )
            .await
            .map_err(db_err)?;
        if let Some(row) = existing.next().await.map_err(db_err)? {
            let id = super::parse_uuid(text(&row, 0)?, "knowledge_job_id")?;
            let status = text(&row, 1)?;
            let lease_until = int(&row, 2)?;
            if status == "running" && lease_until > now {
                return Ok(id);
            }
            let payload = serde_json::to_string(&job)?;
            self.conn
                .execute(
                    "UPDATE knowledge_pipeline_jobs
                     SET job_json = ?1, status = 'pending', run_after = ?2, lease_until = 0,
                         attempts = 0, last_error = NULL, updated_at = ?2
                     WHERE id = ?3",
                    params_from_iter([Value::Text(payload), Value::Integer(now), uuid_value(id)]),
                )
                .await
                .map_err(db_err)?;
            return Ok(id);
        }

        let id = Uuid::new_v4();
        let payload = serde_json::to_string(&job)?;
        self.conn
            .execute(
                "INSERT INTO knowledge_pipeline_jobs
                    (id, job_json, dedup_key, status, run_after, lease_until, attempts,
                     max_attempts, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, 0, 0, ?5, ?4, ?4)",
                params_from_iter([
                    uuid_value(id),
                    Value::Text(payload),
                    Value::Text(dedup_key),
                    Value::Integer(now),
                    Value::Integer(DEFAULT_MAX_ATTEMPTS),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(id)
    }

    async fn claim_due(&self, now: i64, limit: usize) -> Result<Vec<QueuedKnowledgeJob>> {
        let rows = self
            .collect(
                "SELECT id, job_json, attempts, max_attempts
                 FROM knowledge_pipeline_jobs
                 WHERE (status = 'pending' AND run_after <= ?1)
                    OR (status = 'running' AND lease_until <= ?1)
                 ORDER BY run_after ASC, id ASC LIMIT ?2",
                vec![Value::Integer(now), Value::Integer(limit as i64)],
                row_to_queued_job,
            )
            .await?;

        for row in &rows {
            self.conn
                .execute(
                    "UPDATE knowledge_pipeline_jobs
                     SET status = 'running', lease_until = ?1,
                         attempts = attempts + 1, updated_at = ?1
                     WHERE id = ?2",
                    params_from_iter([Value::Integer(now + LEASE_MS), uuid_value(row.id)]),
                )
                .await
                .map_err(db_err)?;
        }

        Ok(rows
            .into_iter()
            .map(|mut row| {
                row.attempts += 1;
                row
            })
            .collect())
    }

    async fn complete(&self, job_id: Uuid, outcome: StageOutcome) -> Result<()> {
        let now = now_ms();
        let (status, run_after, lease_until, last_error) = match &outcome.status {
            StageStatus::Completed => ("succeeded", 0_i64, 0_i64, None),
            StageStatus::RetryLater { after_secs, reason } => (
                "pending",
                now.saturating_add(
                    i64::try_from(*after_secs)
                        .unwrap_or(i64::MAX / 1000)
                        .saturating_mul(1000),
                ),
                0,
                Some(reason.as_str()),
            ),
            StageStatus::Failed { reason } => ("failed", 0, 0, Some(reason.as_str())),
        };
        self.conn
            .execute(
                "UPDATE knowledge_pipeline_jobs
                 SET status = ?1, run_after = ?2, lease_until = ?3, last_error = ?4,
                     updated_at = ?5
                 WHERE id = ?6",
                params_from_iter([
                    Value::Text(status.to_string()),
                    Value::Integer(run_after),
                    Value::Integer(lease_until),
                    last_error
                        .map(|value| Value::Text(value.to_string()))
                        .unwrap_or(Value::Null),
                    Value::Integer(now),
                    uuid_value(job_id),
                ]),
            )
            .await
            .map_err(db_err)?;

        for job in outcome.next_jobs {
            self.enqueue(job).await?;
        }
        Ok(())
    }
}

fn row_to_queued_job(row: &turso::Row) -> Result<QueuedKnowledgeJob> {
    let attempts = int(row, 2)?;
    let max_attempts = int(row, 3)?;
    Ok(QueuedKnowledgeJob {
        id: super::parse_uuid(text(row, 0)?, "knowledge_job_id")?,
        job: serde_json::from_str(&text(row, 1)?)?,
        attempts: u32::try_from(attempts.max(0))
            .map_err(|e| Error::Storage(format!("parse knowledge job attempts: {e}")))?,
        max_attempts: u32::try_from(max_attempts.max(1))
            .map_err(|e| Error::Storage(format!("parse knowledge job max attempts: {e}")))?,
    })
}
