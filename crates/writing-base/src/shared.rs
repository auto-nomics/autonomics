//! Process-level shared writing infrastructure.
//!
//! [`WritingShared`] is opened exactly once per process and cheaply cloned
//! to every agent. It bundles the [`WritingStore`] connection, an optional
//! [`BibShared`](bib_base::BibShared) for citation resolution, and an
//! optional [`LatexEngine`] for compilation.

use std::sync::Arc;

use bib_base::BibShared;

use crate::compile::{LatexEngine, NullEngine, XelatexEngine};
use crate::store::WritingStore;
use crate::{Error, Result};

/// Process-level shared writing resources — created once, cloned per agent.
#[derive(Clone)]
pub struct WritingShared {
    /// Writing document store (Turso connection — cheap Arc clone).
    pub store: Arc<WritingStore>,
    /// Optional bibliography shared instance for citation resolution.
    pub bib: Option<Arc<BibShared>>,
    /// Optional LaTeX compilation engine.
    pub engine: Option<Arc<dyn LatexEngine>>,
}

impl WritingShared {
    /// Open all writing resources from a database path.
    pub async fn open(db_path: impl AsRef<str>) -> Result<Self> {
        let store = WritingStore::open(db_path).await?;
        Ok(Self {
            store: Arc::new(store),
            bib: None,
            engine: None,
        })
    }

    /// Open with an existing `BibShared` for citation resolution and
    /// auto-detect the best LaTeX engine.
    pub async fn open_with(
        db_path: impl AsRef<str>,
        bib: Option<Arc<BibShared>>,
    ) -> Result<Self> {
        let store = WritingStore::open(db_path).await?;

        // Auto-detect engine: prefer XeLaTeX (CJK-capable), fallback to Null.
        let engine: Arc<dyn LatexEngine> = {
            let x = XelatexEngine::new();
            if x.is_available() {
                Arc::new(x)
            } else {
                Arc::new(NullEngine)
            }
        };

        Ok(Self {
            store: Arc::new(store),
            bib,
            engine: Some(engine),
        })
    }

    /// Open an in-memory instance (for tests).
    pub async fn open_in_memory() -> Result<Self> {
        let store = WritingStore::open_in_memory().await?;
        Ok(Self {
            store: Arc::new(store),
            bib: None,
            engine: Some(Arc::new(NullEngine)),
        })
    }
}
