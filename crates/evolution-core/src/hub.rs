//! The process-wide observation hub: the single upstream owner of
//! observation records and routes.
//!
//! Observations are shared evidence — skills and plugins are both
//! downstream consumers. Neither owns the ledger; this hub does. It
//! is installed once per process (see [`ObservationHub::init`]) and
//! every subsystem — the skill manager, plugin RSI, agent tools,
//! gateway handlers — resolves the same state directory through it,
//! so evidence recorded anywhere is visible everywhere.
//!
//! Like the store it wraps, the hub is a thin, stateless view: the
//! state directory on disk is the real shared store, keyed by
//! content-hashed ids so concurrent processes never corrupt each
//! other. The singleton exists to make "which directory" a
//! process-level decision instead of a per-caller guess.

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwapOption;

use crate::error::Result;
use crate::observation::{Observation, ObservationInput, ObservationStore};
use crate::routing::ObservationRouteStore;

/// Process-wide observation hub instance. Replaced only by an
/// explicit re-`init` (tests swap in isolated state dirs this way).
static GLOBAL: ArcSwapOption<ObservationHub> = ArcSwapOption::const_empty();

/// The process-wide owner of observation records and routes.
#[derive(Debug, Clone)]
pub struct ObservationHub {
    state_dir: PathBuf,
}

impl ObservationHub {
    /// Install the process-wide hub. Returns the installed `Arc`.
    /// Calling again replaces the instance — the same re-init
    /// contract the skill manager follows, so isolated tests can
    /// point both at one temp state dir.
    pub fn init(hub: ObservationHub) -> Arc<Self> {
        let arc = Arc::new(hub);
        GLOBAL.store(Some(Arc::clone(&arc)));
        arc
    }

    /// The process-wide hub, lazily constructed from the environment
    /// (`$AUTONOMICS_STATE_DIR` or `~/.autonomics`) when [`init`]
    /// was never called.
    pub fn global() -> Arc<Self> {
        if let Some(existing) = GLOBAL.load().as_ref() {
            return Arc::clone(existing);
        }
        Self::init(Self::from_env())
    }

    /// A hub over an explicit state directory. Does NOT install the
    /// global instance — use [`init`] for that.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
        }
    }

    /// Default from the environment (see [`global`]). Mirrors the
    /// skill manager's resolution so both land on the same directory.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(default_state_dir())
    }

    /// The state directory every store below is rooted at.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// The observation store view under this hub's state dir.
    pub fn observations(&self) -> ObservationStore {
        ObservationStore::open(&self.state_dir)
    }

    /// The route ledger view under this hub's state dir.
    pub fn routes(&self) -> ObservationRouteStore {
        ObservationRouteStore::open(&self.state_dir)
    }

    /// Record one observation (idempotent on content). This is the
    /// canonical entry every producer lands on — agent tools,
    /// automatic failure capture, eval capture, and plugin feedback.
    ///
    /// Downstream notification is the caller's concern: the hub is a
    /// pure ledger and knows nothing about its consumers.
    pub fn record(&self, input: ObservationInput) -> Result<Observation> {
        self.observations().record(input)
    }
}

/// `$AUTONOMICS_STATE_DIR`, else `~/.autonomics`. Kept in sync with
/// the skill manager's resolution on purpose: a mismatch would split
/// the evidence ledger in two.
fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AUTONOMICS_STATE_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".autonomics")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationKind, ObservationSource};

    #[test]
    fn record_is_visible_through_a_freshly_opened_view() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let observation = hub
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: "summary".into(),
                body: "body".into(),
                node_kind: None,
                error: None,
            })
            .unwrap();
        // A brand-new store view over the same directory sees it —
        // the directory is the store, the hub only names it.
        assert!(
            ObservationStore::open(tmp.path())
                .list()
                .iter()
                .any(|o| o.id == observation.id)
        );
    }

    #[test]
    fn init_replaces_the_global_instance() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let hub = ObservationHub::init(ObservationHub::new(first.path()));
        assert_eq!(hub.state_dir(), first.path());
        let hub = ObservationHub::init(ObservationHub::new(second.path()));
        assert_eq!(ObservationHub::global().state_dir(), second.path());
        assert_eq!(hub.state_dir(), second.path());
    }
}
