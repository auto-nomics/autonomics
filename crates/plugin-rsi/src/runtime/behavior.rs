//! Host-bound behavior contracts. Expected values are never passed into the container.
use crate::{Error, Result, TestRunRequest, TestRunResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorSuite {
    pub id: String,
    pub authority: String,
    pub cases: Vec<BehaviorCase>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorCase {
    pub id: String,
    pub kind: String,
    pub inputs: BTreeMap<String, String>,
    pub params: Value,
    pub checks: Vec<OutputCheck>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputCheck {
    pub path: String,
    #[serde(default)]
    pub exact: Option<String>,
    #[serde(default)]
    pub contains: Vec<String>,
    /// Unquoted TSV: exact groups, headers and nonnumeric values; tolerant finite numbers.
    #[serde(default)]
    pub expected_tsv: Option<String>,
    #[serde(default)]
    pub group_columns: Vec<String>,
    #[serde(default)]
    pub relative_tolerance: f64,
    #[serde(default)]
    pub absolute_tolerance: f64,
}
impl BehaviorSuite {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || !["author", "independent"].contains(&self.authority.as_str())
            || self.cases.is_empty()
        {
            return Err(Error::Validation(
                "behavior suite needs id, author/independent authority and nonempty cases".into(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for c in &self.cases {
            if c.id.is_empty() || !ids.insert(&c.id) || c.checks.is_empty() {
                return Err(Error::Validation(
                    "behavior cases need unique ids and nonempty checks".into(),
                ));
            }
            for ch in &c.checks {
                if ch.path.is_empty()
                    || ch.relative_tolerance < 0.0
                    || ch.absolute_tolerance < 0.0
                    || !ch.relative_tolerance.is_finite()
                    || !ch.absolute_tolerance.is_finite()
                {
                    return Err(Error::Validation("invalid behavior output check".into()));
                }
            }
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).map_err(|e| Error::Validation(e.to_string()))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

pub fn check_outputs(case: &BehaviorCase, run: &TestRunResult) -> Result<()> {
    if run.exit_code != 0 {
        return Err(Error::Validation(format!(
            "behavior {} exited {}",
            case.id, run.exit_code
        )));
    }
    for check in &case.checks {
        let output = run
            .outputs
            .iter()
            .find(|o| o.path == check.path)
            .ok_or_else(|| Error::Validation(format!("missing output {}", check.path)))?;
        if !output.present || output.truncated {
            return Err(Error::Validation(format!(
                "missing/truncated output {}",
                check.path
            )));
        }
        let actual = output
            .content
            .as_deref()
            .ok_or_else(|| Error::Validation("output unavailable".into()))?;
        if check
            .exact
            .as_deref()
            .is_some_and(|expected| expected != actual)
            || check.contains.iter().any(|s| !actual.contains(s))
        {
            return Err(Error::Validation(format!("text mismatch {}", check.path)));
        }
        if let Some(expected) = &check.expected_tsv {
            compare_tsv(expected, actual, check)?;
        }
    }
    Ok(())
}
fn compare_tsv(expected: &str, actual: &str, check: &OutputCheck) -> Result<()> {
    fn table(
        text: &str,
        groups: &[String],
    ) -> Result<(Vec<String>, BTreeMap<Vec<String>, Vec<String>>)> {
        let mut lines = text.lines();
        let header: Vec<String> = lines
            .next()
            .unwrap_or("")
            .split('\t')
            .map(str::to_owned)
            .collect();
        let unique: std::collections::BTreeSet<_> = header.iter().collect();
        if header[0].is_empty() || unique.len() != header.len() {
            return Err(Error::Validation("invalid TSV header".into()));
        }
        let keys: Vec<usize> = groups
            .iter()
            .map(|g| {
                header
                    .iter()
                    .position(|c| c == g)
                    .ok_or_else(|| Error::Validation(format!("missing group column {g}")))
            })
            .collect::<Result<_>>()?;
        let mut rows = BTreeMap::new();
        for (i, line) in lines.enumerate() {
            let cells: Vec<String> = line.split('\t').map(str::to_owned).collect();
            if cells.len() != header.len() {
                return Err(Error::Validation("invalid TSV row width".into()));
            }
            let key = if keys.is_empty() {
                vec![i.to_string()]
            } else {
                keys.iter().map(|&k| cells[k].clone()).collect()
            };
            if rows.insert(key, cells).is_some() {
                return Err(Error::Validation("duplicate TSV group".into()));
            }
        }
        Ok((header, rows))
    }
    let (eh, er) = table(expected, &check.group_columns)?;
    let (ah, ar) = table(actual, &check.group_columns)?;
    if eh != ah || er.keys().ne(ar.keys()) {
        return Err(Error::Validation("TSV schema/groups mismatch".into()));
    }
    for (key, row) in er {
        for (col, (e, a)) in row.iter().zip(&ar[&key]).enumerate() {
            let equal = if check.group_columns.contains(&eh[col]) {
                e == a
            } else {
                match (e.parse::<f64>(), a.parse::<f64>()) {
                    (Ok(e), Ok(a)) => {
                        e.is_finite()
                            && a.is_finite()
                            && (e - a).abs()
                                <= check
                                    .absolute_tolerance
                                    .max(check.relative_tolerance * e.abs())
                    }
                    _ => e == a,
                }
            };
            if !equal {
                return Err(Error::Validation(format!(
                    "TSV mismatch {:?}:{}",
                    key, eh[col]
                )));
            }
        }
    }
    Ok(())
}

impl crate::PluginInfra {
    fn behavior_path(&self, plugin: &str) -> Result<std::path::PathBuf> {
        crate::validate_plugin_name(plugin)?;
        Ok(self
            .store()
            .registry_path()
            .parent()
            .ok_or_else(|| Error::Validation("no state directory".into()))?
            .join("behavior-contracts")
            .join(format!("{plugin}.json")))
    }
    /// Host-only registration. Agents have no write API to this binding.
    pub fn configure_behavior_suite(&self, plugin: &str, suite: &BehaviorSuite) -> Result<()> {
        let _guard = self
            .activation_lock
            .lock()
            .map_err(|_| Error::Validation("activation lock poisoned".into()))?;
        suite.validate()?;
        let path = self.behavior_path(plugin)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(suite).map_err(|e| Error::Validation(e.to_string()))?,
        )?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    pub(crate) fn behavior_suite(&self, plugin: &str) -> Result<Option<BehaviorSuite>> {
        let path = self.behavior_path(plugin)?;
        match std::fs::read(path) {
            Ok(bytes) => {
                let suite: BehaviorSuite =
                    serde_json::from_slice(&bytes).map_err(|e| Error::Validation(e.to_string()))?;
                suite.validate()?;
                Ok(Some(suite))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub(crate) fn bind_author_suite_if_present(&self, plugin: &str) -> Result<()> {
        if self.behavior_suite(plugin)?.is_some() {
            return Ok(());
        }
        let workspace = crate::PluginWorkspace::new(self.store().root().join(plugin));
        if !workspace.path().join("fixtures/behavior.json").exists() {
            return Ok(());
        }
        let mut suite: BehaviorSuite =
            serde_json::from_str(&workspace.read_text("fixtures/behavior.json")?)
                .map_err(|e| Error::Validation(e.to_string()))?;
        suite.authority = "author".into();
        suite.validate()?;
        let path = self.behavior_path(plugin)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&suite).map_err(|e| Error::Validation(e.to_string()))?,
        )?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    pub(crate) fn validate_behavior_candidate(
        &self,
        plugin: &str,
        candidate: &Path,
        suite: Option<&BehaviorSuite>,
    ) -> Result<Value> {
        let Some(suite) = suite else {
            return Ok(
                json!({"verification":"unverified","candidate_fingerprint":crate::content_fingerprint(candidate)?}),
            );
        };
        let sessions = self.dev_sessions()?;
        let mut results = Vec::new();
        // Called from the install tool's blocking worker, never from an async worker.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Validation(e.to_string()))?;
        for case in &suite.cases {
            let request = TestRunRequest {
                kind: case.kind.clone(),
                inputs: case.inputs.clone(),
                params: case.params.clone(),
            };
            let outcome = runtime.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(125),
                    sessions.test_run_candidate(plugin, candidate, &request),
                )
                .await
                .map_err(|_| Error::Validation("behavior test timeout".into()))?
            });
            let result = outcome.and_then(|run| {
                check_outputs(case, &run)?;
                Ok(json!({"id":case.id,"passed":true,"execution":run}))
            });
            match result {
                Ok(value) => results.push(value),
                Err(e) => {
                    results.push(json!({"id":case.id,"passed":false,"error":e.to_string()}));
                }
            }
        }
        let report = json!({"verification":if suite.authority=="independent" {"independent_contract"}else{"author_tested"},"suite_id":suite.id,"suite_digest":suite.digest()?,"checker_version":1,"candidate_fingerprint":crate::content_fingerprint(candidate)?,"cases":results,"passed":results.iter().all(|r|r["passed"]==true)});
        let manifest: container_plugin::manifest::PluginManifest =
            toml::from_str(&std::fs::read_to_string(candidate.join("manifest.toml"))?)
                .map_err(|e| Error::Validation(e.to_string()))?;
        let mut report = report;
        report["image_reference"] = json!(manifest.image.reference.as_str());
        report["candidate_tree_digest"] = json!(
            candidate
                .file_name()
                .map(|s| format!("sha256:{}", s.to_string_lossy()))
        );
        let report_path = candidate.with_extension("behavior.json");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Validation(e.to_string()))?
            .as_nanos();
        std::fs::write(
            candidate.with_extension(format!("behavior-{stamp}.json")),
            serde_json::to_vec_pretty(&report).map_err(|e| Error::Validation(e.to_string()))?,
        )?;
        std::fs::write(
            report_path,
            serde_json::to_vec_pretty(&report).map_err(|e| Error::Validation(e.to_string()))?,
        )?;
        if report["passed"] != true {
            return Err(Error::Validation(format!(
                "candidate behavior checks failed: {report}"
            )));
        }
        Ok(report)
    }
    pub fn plugin_status(&self, plugin: &str) -> Result<Value> {
        crate::validate_plugin_name(plugin)?;
        let dev = self.store().root().join(plugin);
        let fingerprint = crate::content_fingerprint(&dev)?;
        let mut active = Value::Null;
        let mut pending = true;
        let mut verification = json!({"verification":"unverified"});
        match self.store().installed_source(plugin) {
            Ok(crate::InstalledPluginSource::Local(source)) => {
                pending = crate::content_fingerprint(&source.path)? != fingerprint;
                let report = source.path.with_extension("behavior.json");
                if report.exists() {
                    verification = serde_json::from_slice(&std::fs::read(report)?)
                        .map_err(|e| Error::Validation(e.to_string()))?;
                }
                active = json!({"commit":source.commit,"digest":source.digest});
            }
            Ok(source) => {
                active = json!({"commit":source.commit(),"source":"git"});
            }
            Err(e) => {
                if self.store().registry_sources()?.contains_key(plugin) {
                    return Err(e);
                }
            }
        }
        if let Some(suite) = self.behavior_suite(plugin)? {
            if verification["suite_digest"].as_str() != Some(suite.digest()?.as_str()) {
                verification = json!({"verification":"requires_revalidation","required_suite_digest":suite.digest()?});
            }
        }
        Ok(
            json!({"plugin_name":plugin,"development_fingerprint":fingerprint,"active":active,"has_unactivated_changes":pending,"behavior":verification,"next_action":if pending || verification["verification"]=="requires_revalidation" {"plugin_install"}else{"run_node"}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check() -> OutputCheck {
        OutputCheck {
            path: "results.tsv".into(),
            exact: None,
            contains: vec![],
            expected_tsv: None,
            group_columns: vec!["type".into()],
            relative_tolerance: 1e-8,
            absolute_tolerance: 1e-10,
        }
    }
    #[test]
    fn checks_groups_counts_na_and_numeric_tolerance() {
        let c = check();
        let expected = "type\tn_dropped_donors\tp\nA\t1\t0.25\nB\t10\tNA\n";
        assert!(
            compare_tsv(
                expected,
                "type\tn_dropped_donors\tp\nB\t10\tNA\nA\t1\t0.25000000001\n",
                &c
            )
            .is_ok()
        );
        for bad in [
            "type\tn_dropped_donors\tp\nA\t0\t0.25\nB\t10\tNA\n",
            "type\tn_dropped_donors\tp\nA\t1\t0.25\nA\t1\t0.25\n",
            "type\tn_dropped_donors\tp\nA\t1\tNaN\nB\t10\tNA\n",
        ] {
            assert!(compare_tsv(expected, bad, &c).is_err());
        }
    }
    #[test]
    fn no_silent_empty_suite_or_unknown_authority() {
        let s = BehaviorSuite {
            id: "x".into(),
            authority: "independent".into(),
            cases: vec![],
        };
        assert!(s.validate().is_err());
    }
}
