//! Export queue: single-worker tokio task driving the configured
//! [`DeckExporter`], writing artifacts into the exports blob area and
//! transitioning the `deck_exports` row. Concurrency 1 — exports are
//! heavy (headless browser each).

use std::sync::Arc;

use crate::assets::BlobArea;
use crate::error::Result;
use crate::export::{DeckExporter, ExportJob};
use crate::store::{DeckStore, ExportStatus};

/// What the worker needs — deliberately narrow so it never borrows the
/// whole [`DeckShared`](crate::DeckShared).
#[derive(Clone)]
pub struct ExportWorker {
    pub store: Arc<DeckStore>,
    pub exports: BlobArea,
    pub exporter: Arc<dyn DeckExporter>,
}

impl ExportWorker {
    /// Spawn the single-worker queue; returns the enqueue sender.
    pub fn spawn(worker: ExportWorker) -> tokio::sync::mpsc::Sender<ExportJob> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExportJob>(32);
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let record = match worker.store.get_export(&job.export_id).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(error = %e, export_id = %job.export_id, "export row vanished");
                        continue;
                    }
                };
                if record.status != ExportStatus::Queued {
                    continue; // cancelled / already done
                }
                let mut record = record;
                record.status = ExportStatus::Running;
                if let Err(e) = worker.store.update_export(&record).await {
                    tracing::warn!(error = %e, "export transition to running failed");
                }
                match worker.exporter.export(job).await {
                    Ok(out) => {
                        let key = crate::assets::export_blob_key(
                            &record.deck_id,
                            &record.id,
                            record.format.as_str(),
                        );
                        match worker.exports.put(&key, out.bytes).await {
                            Ok(()) => {
                                let size = worker
                                    .exports
                                    .get(&key)
                                    .await
                                    .map(|b| b.len() as i64)
                                    .unwrap_or(0);
                                record.status = ExportStatus::Done;
                                record.artifact_key = Some(key);
                                record.artifact_mime = Some(out.mime);
                                record.artifact_size = Some(size);
                                record.error = None;
                            }
                            Err(e) => {
                                record.status = ExportStatus::Error;
                                record.error = Some(format!("artifact store: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        record.status = ExportStatus::Error;
                        record.error = Some(e.to_string());
                    }
                }
                if let Err(e) = worker.store.update_export(&record).await {
                    tracing::warn!(error = %e, "export completion update failed");
                }
            }
        });
        tx
    }
}

/// Result type alias for future cancellation support.
pub type EnqueueResult = Result<()>;
