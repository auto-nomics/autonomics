use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, request::unix_now};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Blocked,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub name: String,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl GateResult {
    pub fn pass(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: GateStatus::Pass,
            reason: None,
        }
    }

    pub fn fail(name: &str, reason: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status: GateStatus::Fail,
            reason: Some(reason.into()),
        }
    }

    pub fn blocked(name: &str, reason: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status: GateStatus::Blocked,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub report_id: String,
    pub proposal_id: String,
    pub created_at: i64,
    pub overall: GateStatus,
    pub gates: Vec<GateResult>,
}

impl ValidationReport {
    pub fn new(proposal_id: &str, attempt: u32, gates: Vec<GateResult>) -> Self {
        let overall = if gates.iter().any(|gate| gate.status == GateStatus::Fail) {
            GateStatus::Fail
        } else if gates.iter().any(|gate| gate.status == GateStatus::Blocked) {
            GateStatus::Blocked
        } else {
            GateStatus::Pass
        };
        Self {
            report_id: format!("attempt-{attempt}"),
            proposal_id: proposal_id.to_string(),
            created_at: unix_now(),
            overall,
            gates,
        }
    }

    pub fn passed(&self) -> bool {
        self.overall == GateStatus::Pass
    }

    /// Append-only persistence: an Agent cannot replace prior evidence.
    pub fn write(&self, reports_dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(reports_dir)?;
        let path = reports_dir.join(format!("{}.json", self.report_id));
        if path.exists() {
            return Err(Error::Validation(format!(
                "report {} already exists",
                path.display()
            )));
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| Error::Validation(error.to_string()))?;
        let tmp = reports_dir.join(format!(".{}.tmp", self.report_id));
        std::fs::write(&tmp, text).map_err(|source| Error::WriteFile {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::WriteFile {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_aggregates_failures_and_is_append_only() {
        let tmp = tempfile::tempdir().unwrap();
        let report = ValidationReport::new(
            "P-test",
            1,
            vec![
                GateResult::pass("manifest"),
                GateResult::fail("policy", "missing README"),
            ],
        );
        assert_eq!(report.overall, GateStatus::Fail);
        let path = report.write(&tmp.path().join("reports")).unwrap();
        assert!(path.is_file());
        assert!(report.write(&tmp.path().join("reports")).is_err());
    }
}
