//! Deck write actor: all mutating operations serialize through one
//! tokio worker (wiki's `WikiHandle` pattern, closure-job form).
//!
//! Why: the direct-handle shape left the read-modify-write cycle
//! (`get_deck_row → apply op → save`) open to interleaving when two
//! agents patch the same deck concurrently. With the actor, that whole
//! sequence runs inside ONE closure on the worker — the queue is the
//! lock. Reads stay direct on the shared handle (no RMW hazard).
//!
//! Job = async closure + oneshot ack. Call sites use [`DeckHandle::write`]
//! and get the operation result back; inside a closure, touch only
//! `DeckShared` internals — NEVER call `handle.write` again (single
//! worker; a nested enqueue deadlocks).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::shared::DeckShared;

/// A serialized unit of write work.
type Job = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Cloneable handle to the deck write worker.
#[derive(Clone)]
pub struct DeckHandle {
    shared: Arc<DeckShared>,
    tx: tokio::sync::mpsc::Sender<Job>,
}

impl DeckHandle {
    /// Spawn the single write worker over `shared`.
    ///
    /// Before entering the loop it sweeps export rows stranded by a
    /// daemon restart: print tokens are stored hashed (raw token never
    /// persisted), so queued/running jobs cannot be re-driven — they
    /// become actionable errors instead of hanging forever.
    pub fn spawn(shared: Arc<DeckShared>) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Job>(64);
        let worker_shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = worker_shared
                .store
                .fail_stale_exports("interrupted by daemon restart; run deck_export again")
                .await
            {
                tracing::warn!(error = %e, "stale export sweep failed");
            }
            while let Some(job) = rx.recv().await {
                // The job captured its Arc<DeckShared> at enqueue time;
                // drive it to completion before taking the next one.
                job.await;
            }
            tracing::debug!("deck write worker stopped (channel closed)");
        });
        Self { shared, tx }
    }

    /// In-memory engine + worker (tests).
    pub async fn spawn_in_memory() -> Result<Self> {
        let shared = Arc::new(DeckShared::open_in_memory().await?);
        Ok(Self::spawn(shared))
    }

    /// Direct read access (load_deck / get_deck_row / blob reads / …).
    ///
    /// Reads may run concurrently with the worker; only operations whose
    /// correctness depends on a read-modify-write cycle must go through
    /// [`DeckHandle::write`].
    pub fn shared(&self) -> &Arc<DeckShared> {
        &self.shared
    }

    /// Enqueue one write closure; resolves with its result once the
    /// worker finished it (preserving order, one at a time).
    ///
    /// Call shape: `handle.write(move |s| Box::pin(async move { … }))`.
    pub async fn write<T>(
        &self,
        f: impl FnOnce(Arc<DeckShared>) -> Pin<Box<dyn Future<Output = Result<T>> + Send>>
        + Send
        + 'static,
    ) -> Result<T>
    where
        T: Send + 'static,
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let shared = self.shared.clone();
        let job: Job = Box::pin(async move {
            let result = f(shared).await;
            // A dropped receiver (caller cancelled) must not kill the worker.
            let _ = ack_tx.send(result);
        });
        self.tx
            .send(job)
            .await
            .map_err(|_| Error::Storage("deck write worker stopped".into()))?;
        match ack_rx.await {
            Ok(result) => result,
            Err(_) => Err(Error::Storage("deck write worker dropped the reply".into())),
        }
    }
}
