//! Execution state shared by workflow schedulers and observers.

use serde::{Deserialize, Serialize};

/// Per-node runtime lifecycle state tracked by the scheduler.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Success,
    Failed,
    /// Not run because an upstream predecessor failed.
    Skipped,
    /// Interrupted by a run-level guard (currently the memory watermark).
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::RuntimeStatus;

    #[test]
    fn legacy_path_and_serialization_remain_compatible() {
        let legacy: crate::dag::runtime::RuntimeStatus = RuntimeStatus::Running;
        assert_eq!(serde_json::to_string(&legacy).unwrap(), "\"running\"");
        assert_eq!(serde_json::from_str::<RuntimeStatus>("\"running\"").unwrap(), legacy);
    }
}
