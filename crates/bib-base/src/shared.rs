//! Process-level shared bibliography infrastructure.
//!
//! [`BibShared`] aggregates the storage handle ([`BibBase`]) and the
//! literature query gateway ([`LiteratureGateway`]) into a single
//! cheaply-cloneable bundle so that every agent spawned by the same
//! `RuntimeHost` can reuse one `Arc<BibBase>` and one
//! `Arc<LiteratureGateway>`.
//!
//! Without this aggregation each `spawn_agent` call would call
//! [`BibBase::open`] + [`LiteratureGateway::with_default_sources`] afresh,
//! opening N independent libSQL connections and N HTTP client stacks for
//! an N-agent network. Sharing one instance avoids that and is safe
//! because both inner types are already designed to be shared behind
//! `Arc` (`BibBase`'s `Connection` is Arc-backed and `LiteratureGateway`
//! is read-only after construction).

use std::path::Path;
use std::sync::Arc;

use crate::bib_base::BibBase;
use crate::query::LiteratureGateway;
use crate::Result;

/// Aggregated, process-level bibliography handle. Cheap to clone
/// (`Arc`-backed).
#[derive(Clone)]
pub struct BibShared {
    /// Storage handle for the bibliography library (articles, collections,
    /// fulltexts, annotations, …).
    pub bib: Arc<BibBase>,
    /// Multi-source literature search/fetch gateway.
    pub gateway: Arc<LiteratureGateway>,
}

impl BibShared {
    /// Open (or create) the bibliography DB at `path` and pair it with a
    /// gateway pre-loaded with all built-in literature sources.
    ///
    /// The DB connection is opened **once** here; subsequent calls share
    /// the same `Arc<BibBase>`. Accepts anything that can be referenced as
    /// a path (`&str`, `String`, `PathBuf`, `&Path`) for caller
    /// ergonomics.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bib = Arc::new(BibBase::open(path.as_ref().to_string_lossy().as_ref()).await?);
        let gateway = Arc::new(LiteratureGateway::with_default_sources());
        Ok(Self { bib, gateway })
    }

    /// Open an in-memory shared bundle (useful for tests).
    pub async fn open_in_memory() -> Result<Self> {
        let bib = Arc::new(BibBase::open_in_memory().await?);
        let gateway = Arc::new(LiteratureGateway::with_default_sources());
        Ok(Self { bib, gateway })
    }
}

impl std::fmt::Debug for BibShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BibShared")
            .field("bib", &"Arc<BibBase>")
            .field("gateway", &"Arc<LiteratureGateway>")
            .finish()
    }
}
