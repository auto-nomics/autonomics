// The abort_task lifecycle tool was removed. This module is kept as a
// placeholder for potential future lifecycle tools; it currently exports
// an empty registration list.

use crate::tools::ToolRegistration;

/// Return lifecycle tool registrations (currently empty).
pub fn lifecycle_registrations() -> Vec<ToolRegistration> {
    vec![]
}
