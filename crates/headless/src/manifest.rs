//! Run manifest — the generic audit artifact of a headless run.
//!
//! Aligned with the project's "typed, auditable harness" claim: every
//! run can leave a manifest recording what was asked, which model
//! answered, and what it cost — independent of any specific benchmark's
//! schema (adapters like reprobio map this onto their own contract).

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::processor::Outcome;
use crate::RunSummary;

/// Identifying metadata the caller contributes (things `run_task` does
/// not know, like the prompt hash).
#[derive(Debug, Clone)]
pub struct ManifestMeta {
    /// Unique id for this invocation; caller-minted (CLI: `Uuid::new_v4`).
    pub run_id: Uuid,
    /// `sha256:<hex>` of the assembled prompt.
    pub prompt_hash: String,
    /// Profile path the agent was spawned from.
    pub profile: String,
    /// Model display name.
    pub model: String,
}

/// The on-disk manifest shape (schema v1).
#[derive(Debug, Serialize)]
pub struct RunManifest {
    pub schema: u32,
    pub run_id: Uuid,
    /// Unix epoch seconds.
    pub created_at: u64,
    pub prompt_hash: String,
    pub profile: String,
    pub model: String,
    pub agent_path: String,
    pub status: &'static str,
    pub turns: u64,
    pub tool_calls: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::event::Usage>,
    pub wall_time_secs: f64,
}

impl RunManifest {
    pub fn new(meta: ManifestMeta, summary: &RunSummary) -> Self {
        Self {
            schema: 1,
            run_id: meta.run_id,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            prompt_hash: meta.prompt_hash,
            profile: meta.profile,
            model: meta.model,
            agent_path: summary.agent_path.clone(),
            status: status_str(summary.outcome),
            turns: summary.turns,
            tool_calls: summary.tool_calls,
            usage: summary.usage,
            wall_time_secs: summary.wall_time_secs,
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("manifest serializes")
    }

    /// Pretty-print to `path`, creating parent directories as needed.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).expect("manifest serializes");
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        writer.write_all(&json)?;
        writer.write_all(b"\n")?;
        Ok(())
    }
}

fn status_str(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::Failed => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Unknown => "unknown",
    }
}

/// `sha256:<hex>` of `prompt` — the manifest's prompt digest format.
pub fn prompt_hash(prompt: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(prompt.as_bytes());
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Usage;

    fn summary() -> RunSummary {
        RunSummary {
            outcome: Outcome::Completed,
            agent_path: "/root/headless".into(),
            profile: "researcher".into(),
            last_message: Some("done".into()),
            wall_time_secs: 1.25,
            usage: Some(Usage {
                input_tokens: Some(11),
                output_tokens: 6,
                ..Default::default()
            }),
            turns: 1,
            tool_calls: 2,
        }
    }

    #[test]
    fn manifest_shape_is_stable() {
        let manifest = RunManifest::new(
            ManifestMeta {
                run_id: Uuid::nil(),
                prompt_hash: prompt_hash("hi"),
                profile: "researcher".into(),
                model: "mock-model".into(),
            },
            &summary(),
        );
        let json = manifest.to_json();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["status"], "completed");
        assert_eq!(json["turns"], 1);
        assert_eq!(json["tool_calls"], 2);
        assert_eq!(json["usage"]["output_tokens"], 6);
        assert_eq!(json["agent_path"], "/root/headless");
        assert!(json["prompt_hash"].as_str().unwrap().starts_with("sha256:"));
        // last_message stays out of the manifest — it is content, not metadata.
        assert!(json.get("last_message").is_none());
    }

    #[test]
    fn prompt_hash_is_deterministic() {
        assert_eq!(prompt_hash("abc"), prompt_hash("abc"));
        assert_ne!(prompt_hash("abc"), prompt_hash("abd"));
    }
}
