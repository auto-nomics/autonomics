//! Shared types for the [`crate::sink`] and
//! [`crate::sink`] nodes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a sink appends to, or overwrites, its destination.
///
/// Used by both file sinks and Iceberg sinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SinkMode {
    /// Add the new rows after whatever is already at the destination.
    Append,
    /// Replace whatever is at the destination with the new rows.
    #[default]
    Overwrite,
}
