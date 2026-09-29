//! Skill evals: run a workflow with fixed params, then check the run
//! report against declared expectations.
//!
//! An eval file `evals/<name>.toml` holds one or more cases; each case
//! picks a workflow template, supplies concrete params (typically
//! pointing at bundled eval fixtures), and declares checks over the
//! run's per-node reports. The vocabulary is deliberately small and
//! covers what the run report already exposes — statuses, row counts,
//! output payload type, output file count, and output file paths.
//! Content-level assertions (reading output files) belong to a later
//! harness revision.
//!
//! ```toml
//! [[case]]
//! name = "smoke"
//! description = "End-to-end regression on the bundled fixture."
//! workflow = "main"
//! [case.params]
//! input = "/evals/fixtures/smoke.csv"
//!
//! [[case.checks]]
//! node = "read"
//! rows_min = 1
//!
//! [[case.checks]]
//! node = "fit"
//! status = "success"
//! rows_max = 1
//! ```
//!
//! [`evaluate`] is pure: it takes the run's node reports as serialized
//! JSON (the engine's `NodeReport` is already agent-friendly) so this
//! crate stays free of DAG-engine dependencies.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SkillError;
use crate::workflow::list_toml_stems;

/// Directory inside a skill holding eval files.
pub const EVALS_DIR: &str = "evals";

/// One declared check. Every field except `node` is optional; a check
/// with only `node` asserts the node ran and succeeded (the engine
/// serializes `RuntimeStatus::Success` as `success`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Check {
    pub node: String,
    /// Expected node status. The engine's `RuntimeStatus` serializes
    /// snake_case (`success`, `failed`, `skipped`, `cancelled`); the
    /// default expectation is `success`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Inclusive lower bound on the primary output's row count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_min: Option<i64>,
    /// Inclusive upper bound on the row count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_max: Option<i64>,
    /// Expected payload type of the first output (e.g. `DataFrame`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_type: Option<String>,
    /// Expected number of output files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<i64>,
    /// Substring expected in at least one output file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path_contains: Option<String>,
}

/// One eval case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Workflow template stem under `workflow/`.
    pub workflow: String,
    /// Concrete parameters for the render.
    #[serde(default)]
    pub params: Value,
    // Field name is the TOML key: `[[case.checks]]`. An earlier
    // rename to "check" silently dropped every check block — unknown
    // TOML keys are ignored — so evals ran with zero checks and
    // passed trivially. Caught by the RSI loop's failing-eval test.
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// Result of one check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckResult {
    pub node: String,
    pub passed: bool,
    /// Human-readable failure reason; empty on pass.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Result of one case.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalReport {
    pub passed: bool,
    pub cases: Vec<CaseResult>,
}

/// Wrapper for deserializing an eval file: a TOML document whose root
/// is a table holding the `[[case]]` array.
#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(rename = "case")]
    cases: Vec<EvalCase>,
}

/// Parse one eval file (which may hold several `[[case]]` entries).
pub fn parse_cases(text: &str) -> Result<Vec<EvalCase>, SkillError> {
    let cases = toml::from_str::<EvalFile>(text)
        .map_err(|e| SkillError::invalid_frontmatter("<evals>", format!("invalid eval TOML: {e}")))?
        .cases;
    if cases.is_empty() {
        return Err(SkillError::invalid_frontmatter(
            "<evals>",
            "eval file declares no [[case]] entries",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for case in &cases {
        if case.name.trim().is_empty() {
            return Err(SkillError::invalid_frontmatter(
                "<evals>",
                "eval case with empty name",
            ));
        }
        if !seen.insert(case.name.clone()) {
            return Err(SkillError::invalid_frontmatter(
                "<evals>",
                format!("duplicate eval case name {:?}", case.name),
            ));
        }
        if case.workflow.trim().is_empty() {
            return Err(SkillError::invalid_frontmatter(
                "<evals>",
                format!("case {:?}: missing workflow", case.name),
            ));
        }
    }
    Ok(cases)
}

/// Eval file stems under `dir/evals/`, sorted. Missing dir → empty.
pub fn eval_stems_in_dir(dir: &Path) -> Vec<String> {
    list_toml_stems(&dir.join(EVALS_DIR))
}

/// Parse `dir/evals/<stem>.toml`.
pub fn load_cases(dir: &Path, stem: &str) -> Result<Vec<EvalCase>, SkillError> {
    let path = dir.join(EVALS_DIR).join(format!("{stem}.toml"));
    let text = std::fs::read_to_string(&path).map_err(|e| SkillError::Unreadable {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    parse_cases(&text).map_err(|e| match e {
        SkillError::InvalidFrontmatter { reason, .. } => {
            SkillError::invalid_frontmatter(path, reason)
        }
        other => other,
    })
}

/// Evaluate one case's checks against the serialized per-node reports
/// of a run (a JSON array of engine `NodeReport`s).
///
/// Checks reference nodes by their **unprefixed** template id — callers
/// that namespace node ids when building the DAG should strip the
/// prefix when serializing reports, or keep ids unprefixed.
pub fn evaluate_case(case: &EvalCase, nodes_json: &[Value]) -> CaseResult {
    let by_id: std::collections::HashMap<&str, &Value> = nodes_json
        .iter()
        .filter_map(|n| {
            let id = n.get("id")?.as_str()?;
            Some((id, n))
        })
        .collect();
    let mut checks = Vec::with_capacity(case.checks.len());
    for check in &case.checks {
        let (passed, reason) = evaluate_check(check, by_id.get(check.node.as_str()).copied());
        checks.push(CheckResult {
            node: check.node.clone(),
            passed,
            reason,
        });
    }
    let passed = checks.iter().all(|c| c.passed);
    CaseResult {
        name: case.name.clone(),
        passed,
        checks,
    }
}

/// Evaluate all cases, pairing each with its run's reports. Pairs are
/// `(case, nodes_json)` in caller-supplied order.
pub fn evaluate_all(pairs: &[(EvalCase, Vec<Value>)]) -> EvalReport {
    let cases: Vec<CaseResult> = pairs
        .iter()
        .map(|(case, nodes)| evaluate_case(case, nodes))
        .collect();
    let passed = cases.iter().all(|c| c.passed);
    EvalReport { passed, cases }
}

fn evaluate_check(check: &Check, report: Option<&Value>) -> (bool, String) {
    let Some(report) = report else {
        return (
            false,
            format!("node {:?} not found in run report", check.node),
        );
    };

    let status = report.get("status").and_then(Value::as_str).unwrap_or("");
    // A check listing only `node` means "ran fine": status defaults to
    // `success` (the engine's snake_case serialization). With other
    // assertions present, status is checked only when explicitly
    // declared.
    let status_only = check.status.is_none()
        && check.rows_min.is_none()
        && check.rows_max.is_none()
        && check.output_type.is_none()
        && check.files.is_none()
        && check.file_path_contains.is_none();
    let expected_status =
        check
            .status
            .as_deref()
            .or(if status_only { Some("success") } else { None });
    if let Some(expected) = expected_status
        && !status.eq_ignore_ascii_case(expected)
    {
        return (
            false,
            format!("status is {status:?}, expected {expected:?}"),
        );
    }

    if let Some(min) = check.rows_min {
        let rows = report.get("output_rows").and_then(Value::as_i64);
        match rows {
            Some(rows) if rows >= min => {}
            other => {
                return (
                    false,
                    format!("output_rows is {other:?}, expected >= {min}"),
                );
            }
        }
    }
    if let Some(max) = check.rows_max {
        let rows = report.get("output_rows").and_then(Value::as_i64);
        match rows {
            Some(rows) if rows <= max => {}
            other => {
                return (
                    false,
                    format!("output_rows is {other:?}, expected <= {max}"),
                );
            }
        }
    }
    if let Some(want_type) = &check.output_type {
        let got = report.get("output_type").and_then(Value::as_str);
        if got != Some(want_type.as_str()) {
            return (
                false,
                format!("output_type is {got:?}, expected {want_type:?}"),
            );
        }
    }
    if let Some(files) = check.files {
        let got = report
            .get("output_files")
            .and_then(Value::as_array)
            .map(|a| a.len() as i64);
        if got != Some(files) {
            return (
                false,
                format!("output_files count is {got:?}, expected {files}"),
            );
        }
    }
    if let Some(needle) = &check.file_path_contains {
        let hit = report
            .get("output_files")
            .and_then(Value::as_array)
            .is_some_and(|files| {
                files.iter().any(|f| {
                    f.get("path")
                        .and_then(Value::as_str)
                        .is_some_and(|p| p.contains(needle.as_str()))
                })
            });
        if !hit {
            return (false, format!("no output file path contains {needle:?}"));
        }
    }
    (true, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, status: &str, rows: Option<i64>, files: Vec<&str>) -> Value {
        json!({
            "id": id,
            "status": status,
            "output_rows": rows,
            "output_files": files
                .iter()
                .map(|p| json!({ "path": p }))
                .collect::<Vec<_>>(),
        })
    }

    fn case(checks: Vec<Check>) -> EvalCase {
        EvalCase {
            name: "t".into(),
            description: String::new(),
            workflow: "main".into(),
            params: json!({}),
            checks,
        }
    }

    #[test]
    fn parses_multi_case_file_with_validation() {
        let text = r#"
[[case]]
name = "smoke"
workflow = "main"
[case.params]
input = "/fixtures/a.csv"
[[case.checks]]
node = "read"
rows_min = 1

[[case]]
name = "empty-input"
workflow = "main"
[[case.checks]]
node = "read"
rows_max = 0
"#;
        let cases = parse_cases(text).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].params["input"], json!("/fixtures/a.csv"));
        // Regression: checks must actually parse (they were silently
        // dropped by a field rename once).
        assert_eq!(cases[0].checks.len(), 1);
        assert_eq!(cases[0].checks[0].rows_min, Some(1));
        assert!(parse_cases("").is_err());
        assert!(parse_cases("[[case]]\nname = \"x\"\nworkflow = \"main\"\n[[case]]\nname = \"x\"\nworkflow = \"main\"\n").is_err());
    }

    #[test]
    fn default_check_is_success() {
        let reports = vec![node("read", "success", Some(3), vec![])];
        let ok = case(vec![Check {
            node: "read".into(),
            ..Default::default()
        }]);
        assert!(evaluate_case(&ok, &reports).passed);

        let failed = vec![node("read", "failed", None, vec![])];
        assert!(!evaluate_case(&ok, &failed).passed);
    }

    #[test]
    fn rows_bounds_and_missing_node() {
        let reports = vec![node("read", "success", Some(3), vec![])];
        let min4 = case(vec![Check {
            node: "read".into(),
            rows_min: Some(4),
            ..Default::default()
        }]);
        assert!(!evaluate_case(&min4, &reports).passed);

        let max2 = case(vec![Check {
            node: "read".into(),
            rows_max: Some(2),
            ..Default::default()
        }]);
        assert!(!evaluate_case(&max2, &reports).passed);

        let ghost = case(vec![Check {
            node: "ghost".into(),
            ..Default::default()
        }]);
        let result = evaluate_case(&ghost, &reports);
        assert!(!result.passed);
        assert!(result.checks[0].reason.contains("not found"));
    }

    #[test]
    fn output_type_files_and_path_contains() {
        let reports = vec![node("out", "success", None, vec!["/vfs/out/result.csv"])];
        let good = case(vec![Check {
            node: "out".into(),
            files: Some(1),
            file_path_contains: Some("result.csv".into()),
            ..Default::default()
        }]);
        assert!(evaluate_case(&good, &reports).passed);

        let bad_files = case(vec![Check {
            node: "out".into(),
            files: Some(2),
            ..Default::default()
        }]);
        assert!(!evaluate_case(&bad_files, &reports).passed);

        let bad_path = case(vec![Check {
            node: "out".into(),
            file_path_contains: Some(".parquet".into()),
            ..Default::default()
        }]);
        assert!(!evaluate_case(&bad_path, &reports).passed);
    }

    #[test]
    fn explicit_non_success_status_is_checkable() {
        let reports = vec![node("may_fail", "failed", None, vec![])];
        let expect_failed = case(vec![Check {
            node: "may_fail".into(),
            status: Some("failed".into()),
            ..Default::default()
        }]);
        assert!(evaluate_case(&expect_failed, &reports).passed);

        // Engine statuses are snake_case; a stale "succeeded" phrasing
        // must fail loudly rather than silently pass everything.
        let stale = case(vec![Check {
            node: "may_fail".into(),
            status: Some("succeeded".into()),
            ..Default::default()
        }]);
        assert!(!evaluate_case(&stale, &reports).passed);
    }

    #[test]
    fn evaluate_all_aggregates() {
        let pairs = vec![
            (
                case(vec![Check {
                    node: "a".into(),
                    ..Default::default()
                }]),
                vec![node("a", "success", None, vec![])],
            ),
            (
                case(vec![Check {
                    node: "b".into(),
                    ..Default::default()
                }]),
                vec![node("b", "failed", None, vec![])],
            ),
        ];
        let report = evaluate_all(&pairs);
        assert!(!report.passed);
        assert_eq!(report.cases.len(), 2);
        assert!(report.cases[0].passed);
        assert!(!report.cases[1].passed);
    }
}
