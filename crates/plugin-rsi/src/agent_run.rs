use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use coding_agent::CodingTask;
use container_plugin::{manifest::PluginManifest, node_definition};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{Error, ProposalWorkspace, Result};

/// Durable evidence for one coding-agent run against a plugin proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingAgentRun {
    pub run_id: String,
    pub proposal_id: String,
    pub agent: String,
    pub argv: Vec<String>,
    pub prompt_sha256: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub synced: bool,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub created_at: i64,
    pub added_files: Vec<String>,
    pub changed_files: Vec<String>,
    pub removed_files: Vec<String>,
}

impl CodingAgentRun {
    pub fn write(&self, reports_dir: &Path) -> Result<PathBuf> {
        let reports_dir = reports_dir.join("coding");
        std::fs::create_dir_all(&reports_dir)?;
        let path = reports_dir.join(format!("{}.json", self.run_id));
        if path.exists() {
            return Err(Error::Validation(format!(
                "run report {} already exists",
                path.display()
            )));
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| Error::Validation(format!("invalid coding run report: {error}")))?;
        let tmp = reports_dir.join(format!(".{}.tmp", self.run_id));
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

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceChanges {
    pub(crate) added: Vec<String>,
    pub(crate) changed: Vec<String>,
    pub(crate) removed: Vec<String>,
}

impl WorkspaceChanges {
    pub(crate) fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

pub(crate) struct AgentRunPaths {
    pub workspace: ProposalWorkspace,
}

pub(crate) fn prepare_agent_workspace(
    destination_root: &Path,
    source: &ProposalWorkspace,
) -> Result<AgentRunPaths> {
    let run_root = destination_root.to_path_buf();
    let workspace_root = run_root.join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let workspace = ProposalWorkspace::new(workspace_root);
    for relative in source.list_files()? {
        workspace.write_text(&relative, &source.read_text(&relative)?)?;
    }
    Ok(AgentRunPaths { workspace })
}

pub(crate) fn workspace_changes(
    source: &ProposalWorkspace,
    candidate: &ProposalWorkspace,
) -> Result<WorkspaceChanges> {
    let before = BTreeSet::from_iter(source.list_files()?);
    let after = BTreeSet::from_iter(candidate.list_files()?);
    let mut changes = WorkspaceChanges::default();
    for relative in &after {
        if !before.contains(relative.as_str()) {
            changes.added.push(relative.clone());
        } else if source.read_text(relative)? != candidate.read_text(relative)? {
            changes.changed.push(relative.clone());
        }
    }
    for relative in before {
        if !after.contains(&relative) {
            changes.removed.push(relative);
        }
    }
    Ok(changes)
}

pub(crate) fn validate_candidate(
    candidate: &ProposalWorkspace,
    plugin_name: &str,
    environment_reference: &str,
) -> Result<PluginManifest> {
    let files = candidate.list_files()?;
    if files.iter().any(|path| {
        Path::new(path)
            .file_name()
            .is_some_and(|name| name == "Dockerfile")
    }) {
        return Err(Error::Validation(
            "coding agent cannot add a Dockerfile; use an approved environment".into(),
        ));
    }
    let manifest: PluginManifest = toml::from_str(&candidate.read_text("manifest.toml")?)
        .map_err(|error| Error::Validation(format!("invalid manifest.toml: {error}")))?;
    if manifest.plugin_name != plugin_name {
        return Err(Error::Validation(
            "coding agent cannot change manifest plugin_name".into(),
        ));
    }
    if manifest.image.reference.as_str() != environment_reference {
        return Err(Error::Validation(
            "coding agent cannot change the bound environment".into(),
        ));
    }
    if !manifest.panels.is_empty() {
        return Err(Error::Validation(
            "coding agent cannot add panels in the MVP".into(),
        ));
    }
    if manifest.nodes.is_empty() {
        return Err(Error::Validation(
            "coding agent cannot remove the last node".into(),
        ));
    }
    let mut kinds = BTreeSet::new();
    for node in &manifest.nodes {
        node_definition::validate(node)
            .map_err(|error| Error::Validation(format!("invalid node `{}`: {error}", node.kind)))?;
        if !kinds.insert(node.kind.clone()) {
            return Err(Error::Validation(format!(
                "duplicate node kind `{}`",
                node.kind
            )));
        }
        if node.command.script.is_some() || node.command.script_file.is_none() {
            return Err(Error::Validation(format!(
                "node `{}` must declare only a safe script_file",
                node.kind
            )));
        }
        let script_file = node.command.script_file.as_deref().expect("checked above");
        candidate.read_text(script_file)?;
    }
    Ok(manifest)
}

pub(crate) fn adopt_candidate(
    source: &ProposalWorkspace,
    candidate: &ProposalWorkspace,
    changes: &WorkspaceChanges,
) -> Result<()> {
    for relative in changes.added.iter().chain(&changes.changed) {
        source.write_text(relative, &candidate.read_text(relative)?)?;
    }
    for relative in &changes.removed {
        source.remove_file(relative)?;
    }
    Ok(())
}

pub(crate) fn plugin_task(task: CodingTask) -> Result<CodingTask> {
    const POLICY: &str = "You are developing a plugin repository. \
         Do not run git, gh, or Docker commands. Do not create a Dockerfile. \
         Keep manifest.toml aligned with every node and its script file.";
    let instructions = format!("{POLICY}\n\n{}", task.instructions);
    let task = CodingTask::new(instructions).with_request_context(task.request_context);
    task.validate()?;
    Ok(task)
}

pub(crate) fn new_run_id(proposal_id: &str, agent: &str, prompt: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(proposal_id.as_bytes());
    hash.update([0u8]);
    hash.update(agent.as_bytes());
    hash.update([0u8]);
    hash.update(prompt.as_bytes());
    hash.update([0u8]);
    hash.update(unix_now_nanos().to_le_bytes());
    format!("run-{:016x}-{}", unix_now(), hex_prefix(&hash.finalize()))
}

pub(crate) fn prompt_hash(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    hex_prefix(&digest).to_string()
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
