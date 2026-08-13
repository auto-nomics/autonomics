//! Partial, idempotent mutation of an existing [`ResourceEntry`].
//!
//! [`ResourcePatch`] is the unit of update passed to
//! [`ResourceCatalog::patch`](crate::catalog::ResourceCatalog::patch). Only
//! fields explicitly set to `Some(_)` are touched; every other field on the
//! existing entry is left untouched. This is the safe alternative to
//! remove-then-re-add, which would clobber `archive_status`, ingestion
//! progress, and any other runtime state currently on the entry.
//!
//! ## Extensibility
//!
//! New fields (e.g. metadata merge, tag add/remove) are added as additional
//! `Option<...>` members. Old callers that constructed the patch with
//! `ResourcePatch::default()` or `ResourcePatch::new()` keep working —
//! empty patches are still rejected with a clear validation error.
//!
//! [`ResourceEntry`]: crate::entry::ResourceEntry

use crate::entry::ResourceEntry;

/// A partial mutation applied to an existing [`ResourceEntry`].
///
/// Construct with the builder API, e.g.:
///
/// ```ignore
/// ResourcePatch::new().description("new description")
/// ```
///
/// [`ResourceEntry`]: crate::entry::ResourceEntry
#[derive(Debug, Default, Clone)]
pub struct ResourcePatch {
    /// If `Some`, replace the entry's description. `None` leaves it alone.
    pub description: Option<String>,
}

impl ResourcePatch {
    /// An empty patch (no fields set).
    ///
    /// Note: an empty patch is rejected by
    /// [`ResourceCatalog::patch`](crate::catalog::ResourceCatalog::patch) —
    /// the caller almost certainly forgot to specify a field.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the description to `d`, replacing whatever was there before.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    /// True when no field has been set — `patch()` rejects this with a
    /// validation error so a silent no-op can't mask a forgotten flag.
    pub(crate) fn is_empty(&self) -> bool {
        self.description.is_none()
    }
}
