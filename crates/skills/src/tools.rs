//! Agent-facing skill tools: `skill_list`, `skill_get`, `skill_search`.
//!
//! Skills are exposed through structured tools rather than the VFS:
//! the tools bound pagination and size caps, keep frontmatter out of
//! the model's way, and centralize access so a later tier can add
//! usage telemetry in one place. All reads are local-directory reads
//! of a few KiB; blocking file IO inside the async `run` is fine.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use serde::Deserialize;

use crate::evolution::SkillCommandOutcome;
use crate::format::MAX_BODY_BYTES;
use crate::manager::SkillManager;
use agentik_core::memory::{KmsSystem, KnowledgeEvent, KnowledgeEvidenceInput, SkillVersion};

// ────────────────────────── inputs ──────────────────────────

#[tool(
    name = "skill_list",
    description = "List installed skills with their tier (builtin/global/workspace) \
        and a one-line description. Each description states when the skill applies. \
        Fetch the full guide with skill_get."
)]
pub struct SkillListInput {
    #[desc = "Optional tier filter: builtin, global, or workspace."]
    pub tier: Option<String>,
}

#[tool(
    name = "skill_get",
    description = "Fetch the full SKILL.md body of one installed skill by its exact \
        name (see skill_list / skill_search). Returns the operational instructions. \
        Line-based pagination: offset is the 1-indexed first line, limit caps the \
        lines returned (default 400, max 2000)."
)]
pub struct SkillGetInput {
    #[desc = "Exact skill name from skill_list / skill_search."]
    pub name: String,
    #[desc = "1-indexed first line to return. Default 1."]
    pub offset: Option<usize>,
    #[desc = "Max lines to return. Default 400, capped at 2000."]
    pub limit: Option<usize>,
}

#[tool(
    name = "skill_search",
    description = "Search installed skills by keyword over names, tags, and \
        descriptions (case-insensitive). Every term must match. Use domain \
        keywords (e.g. 'gwas', 'imputation', 'literature') before starting \
        multi-step work to check whether a skill already covers it."
)]
pub struct SkillSearchInput {
    #[desc = "Free-text query; whitespace-separated terms are ANDed."]
    pub query: String,
}

// ────────────────────────── tools ──────────────────────────

pub struct SkillListTool {
    pub manager: Arc<SkillManager>,
    pub kms: Option<Arc<KmsSystem>>,
}

#[derive(Debug, Clone)]
struct SkillProjection {
    name: String,
    description: String,
    tags: Vec<String>,
    tier: String,
    body: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SkillVersionMetadata {
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    tier: String,
}

fn kms_projection(version: &SkillVersion) -> Option<SkillProjection> {
    let metadata = serde_json::from_str::<SkillVersionMetadata>(&version.metadata_json).ok()?;
    Some(SkillProjection {
        name: version.skill_name.clone(),
        description: metadata.description,
        tags: metadata.tags,
        tier: metadata.tier,
        body: version.body_md.clone(),
    })
}

async fn kms_projections(kms: Option<&Arc<KmsSystem>>, limit: usize) -> Vec<SkillProjection> {
    let Some(kms) = kms else {
        return Vec::new();
    };
    let store = kms.store();
    match store.list_skill_versions(limit).await {
        Ok(versions) => versions
            .iter()
            .filter(|version| version.status == "active")
            .filter_map(kms_projection)
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "cannot read KMS skill projection");
            Vec::new()
        }
    }
}

async fn record_kms_usage(kms: &KmsSystem, name: &str, kind: crate::UsageKind) {
    let input = KnowledgeEvidenceInput {
        source_kind: "skill_usage".to_string(),
        source_id: format!(
            "{name}:{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ),
        evidence_kind: kind.as_str().to_string(),
        summary: format!("skill {name} recorded {} usage", kind.as_str()),
        body: format!(
            "Skill {name:?} was read through the skill tools ({}). This is fitness \
             evidence for procedure consolidation and diagnosis.",
            kind.as_str()
        ),
        anchor_path: Some(format!("/procedures/{name}")),
        error: None,
    };
    if let Err(error) = kms
        .submit_event(KnowledgeEvent::ObservationRecorded { input })
        .await
    {
        tracing::warn!(skill = name, kind = kind.as_str(), %error, "cannot queue skill usage evidence");
    }
}

fn projection_line(projection: &SkillProjection) -> String {
    let tags = if projection.tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", projection.tags.join(", "))
    };
    format!(
        "- {} ({}){} — {}\n",
        projection.name,
        projection.tier,
        tags,
        crate::format::prompt_safe_description(&projection.description, 240)
    )
}

#[async_trait::async_trait]
impl ToolFunction for SkillListTool {
    type Input = SkillListInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let projections = kms_projections(self.kms.as_ref(), 10_000).await;
        if let Some(tier) = input
            .tier
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let filtered = projections
                .iter()
                .filter(|projection| projection.tier == tier)
                .collect::<Vec<_>>();
            if !filtered.is_empty() {
                let mut out = format!("{} skill(s):\n", filtered.len());
                for projection in filtered {
                    out.push_str(&projection_line(projection));
                }
                return Ok(ToolResult::success(out));
            }
        } else if !projections.is_empty() {
            let mut out = format!("{} skill(s):\n", projections.len());
            for projection in &projections {
                out.push_str(&projection_line(projection));
            }
            return Ok(ToolResult::success(out));
        }

        let entries = self.manager.registry().list();
        if entries.is_empty() {
            return Ok(ToolResult::success(
                "No skills installed. Ask the user to install one with \
                 `autonomics-skills install <source>`.",
            ));
        }
        let mut out = format!("{} skill(s):\n", entries.len());
        for entry in &entries {
            let tags = if entry.meta.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", entry.meta.tags.join(", "))
            };
            out.push_str(&format!(
                "- {} ({}){} — {}\n",
                entry.meta.name,
                entry.tier.as_str(),
                tags,
                crate::format::prompt_safe_description(&entry.meta.description, 240)
            ));
        }
        Ok(ToolResult::success(out))
    }
}

pub struct SkillGetTool {
    pub manager: Arc<SkillManager>,
    pub kms: Option<Arc<KmsSystem>>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillGetTool {
    type Input = SkillGetInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Ok(ToolResult::error("skill_get: 'name' must be non-empty"));
        }
        let projection = kms_projections(self.kms.as_ref(), 10_000)
            .await
            .into_iter()
            .find(|projection| projection.name == name);
        let (name, tier, body) = if let Some(projection) = projection {
            (projection.name, projection.tier, projection.body)
        } else {
            let Some(doc) = self.manager.registry().get(name).unwrap_or_else(|e| {
                tracing::warn!(skill = name, error = %e, "skill_get: unreadable skill");
                None
            }) else {
                let names: Vec<String> = self
                    .manager
                    .registry()
                    .list()
                    .into_iter()
                    .map(|e| e.meta.name)
                    .collect();
                return Ok(ToolResult::error(format!(
                    "skill_get: no skill named {name:?}. Installed: {}",
                    names.join(", ")
                )));
            };
            (doc.meta.name, doc.tier.as_str().to_string(), doc.body)
        };

        self.manager.record_usage(&name, crate::UsageKind::Get);
        if let Some(kms) = &self.kms {
            record_kms_usage(kms, &name, crate::UsageKind::Get).await;
        }
        let lines: Vec<&str> = body.lines().collect();
        let offset = input.offset.unwrap_or(1).max(1);
        let limit = input.limit.unwrap_or(400).min(2000);
        let start = (offset - 1).min(lines.len());
        let end = (start + limit).min(lines.len());
        let mut page: Vec<&str> = lines[start..end].to_vec();
        let mut truncated_flag = false;
        // Enforce the byte cap on the materialized page by dropping
        // trailing lines until it fits.
        while page.iter().map(|l| l.len() + 1).sum::<usize>() > MAX_BODY_BYTES && page.len() > 1 {
            page.pop();
            truncated_flag = true;
        }

        let mut out = format!("# {name} ({tier})\n\n");
        for line in &page {
            out.push_str(line);
            out.push('\n');
        }
        if end < lines.len() || truncated_flag {
            out.push_str(&format!(
                "\n[lines {}-{} of {} — continue with offset={}]",
                start + 1,
                start + page.len(),
                lines.len(),
                start + page.len() + 1
            ));
        }
        Ok(ToolResult::success(out))
    }
}

pub struct SkillSearchTool {
    pub manager: Arc<SkillManager>,
    pub kms: Option<Arc<KmsSystem>>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillSearchTool {
    type Input = SkillSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let terms: Vec<String> = input
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let projections = kms_projections(self.kms.as_ref(), 10_000)
            .await
            .into_iter()
            .filter(|projection| {
                terms.iter().all(|term| {
                    projection.name.to_lowercase().contains(term)
                        || projection.description.to_lowercase().contains(term)
                        || projection
                            .tags
                            .iter()
                            .any(|tag| tag.to_lowercase().contains(term))
                })
            })
            .collect::<Vec<_>>();
        if !projections.is_empty() {
            for projection in &projections {
                self.manager
                    .record_usage(&projection.name, crate::UsageKind::SearchHit);
                if let Some(kms) = &self.kms {
                    record_kms_usage(kms, &projection.name, crate::UsageKind::SearchHit).await;
                }
            }
            let mut out = format!("{} match(s) for {:?}:\n", projections.len(), input.query);
            for projection in &projections {
                out.push_str(&projection_line(projection));
            }
            return Ok(ToolResult::success(out));
        }

        let entries = self.manager.registry().search(&input.query);
        for entry in &entries {
            self.manager
                .record_usage(&entry.meta.name, crate::UsageKind::SearchHit);
        }
        if entries.is_empty() {
            return Ok(ToolResult::success(format!(
                "No skills match {:?}. Try broader domain keywords, or \
                 skill_list for the full library.",
                input.query
            )));
        }
        let mut out = format!("{} match(s) for {:?}:\n", entries.len(), input.query);
        for entry in &entries {
            out.push_str(&format!(
                "- {} ({}) — {}\n",
                entry.meta.name,
                entry.tier.as_str(),
                crate::format::prompt_safe_description(&entry.meta.description, 240)
            ));
        }
        Ok(ToolResult::success(out))
    }
}

// ────────────────────────── workflows ──────────────────────────

#[tool(
    name = "skill_workflows",
    description = "List the workflow templates bundled by one skill, with each \
        template's description and parameter schema (name, type, required, \
        default). Use before skill_run_workflow to see exactly which params \
        to supply. Only tier-backed skills (global/workspace) carry \
        workflows."
)]
pub struct SkillWorkflowsInput {
    #[desc = "Exact skill name from skill_list / skill_search."]
    pub name: String,
}

pub struct SkillWorkflowsTool {
    pub manager: Arc<SkillManager>,
    pub kms: Option<Arc<KmsSystem>>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillWorkflowsTool {
    type Input = SkillWorkflowsInput;

    async fn run(&self, input: SkillWorkflowsInput) -> Result<ToolResult, ToolError> {
        let name = input.name.trim();
        if let Some(system) = &self.kms
            && let Some(version) = system
                .store()
                .list_skill_versions(10_000)
                .await
                .ok()
                .and_then(|versions| {
                    versions
                        .into_iter()
                        .find(|version| version.status == "active" && version.skill_name == name)
                })
        {
            let artifacts = match system.store().list_skill_artifacts(version.id).await {
                Ok(artifacts) => artifacts,
                Err(error) => {
                    return Ok(ToolResult::error(format!(
                        "skill_workflows: cannot read KMS artifacts: {error}"
                    )));
                }
            };
            let workflows = artifacts
                .iter()
                .filter(|artifact| {
                    artifact
                        .relative_path
                        .strip_prefix("workflow/")
                        .is_some_and(|path| path.ends_with(".toml"))
                })
                .collect::<Vec<_>>();
            let mut out = format!("Skill {name:?} bundles {} workflow(s):\n", workflows.len());
            for artifact in &workflows {
                let stem = artifact
                    .relative_path
                    .trim_start_matches("workflow/")
                    .trim_end_matches(".toml");
                append_workflow(
                    &mut out,
                    stem,
                    crate::workflow::WorkflowTemplate::from_toml(&artifact.content),
                );
            }
            if workflows.is_empty() {
                out.push_str(&format!("\nSkill {name:?} bundles no workflow templates."));
            } else {
                out.push_str("\nInstantiate with skill_run_workflow (skill, workflow, params).");
            }
            return Ok(ToolResult::success(out));
        }

        let Some(doc) = self.manager.registry().get(name).unwrap_or_else(|e| {
            tracing::warn!(skill = name, error = %e, "skill_workflows: unreadable skill");
            None
        }) else {
            return Ok(ToolResult::error(format!(
                "skill_workflows: no skill named {name:?}"
            )));
        };
        if doc.workflows.is_empty() {
            return Ok(ToolResult::success(format!(
                "Skill {name:?} bundles no workflow templates."
            )));
        }
        let Some(dir) = &doc.dir else {
            return Ok(ToolResult::success(format!(
                "Skill {name:?} is builtin; workflows require a tier-backed skill."
            )));
        };
        let mut out = format!(
            "Skill {name:?} bundles {} workflow(s):\n",
            doc.workflows.len()
        );
        for stem in &doc.workflows {
            append_workflow(
                &mut out,
                stem,
                crate::workflow::WorkflowTemplate::load(dir, stem),
            );
        }
        out.push_str("\nInstantiate with skill_run_workflow (skill, workflow, params).");
        Ok(ToolResult::success(out))
    }
}

fn append_workflow(
    out: &mut String,
    stem: &str,
    template: Result<crate::workflow::WorkflowTemplate, crate::error::SkillError>,
) {
    match template {
        Ok(template) => {
            out.push_str(&format!("\nworkflow {stem:?} — {}\n", template.description));
            if template.params.is_empty() {
                out.push_str("  params: none\n");
            } else {
                out.push_str("  params:\n");
                for (pname, spec) in &template.params {
                    let required = if spec.required {
                        "required"
                    } else {
                        "optional"
                    };
                    let default = spec
                        .default
                        .as_ref()
                        .map(|d| format!(", default {d}"))
                        .unwrap_or_default();
                    let desc = if spec.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", spec.description)
                    };
                    out.push_str(&format!(
                        "  - {pname}: {} ({required}{default}){desc}\n",
                        spec.kind
                    ));
                }
            }
            out.push_str(&format!(
                "  nodes: {} ({})\n",
                template.nodes.len(),
                template
                    .nodes
                    .iter()
                    .map(|n| n.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Err(e) => {
            out.push_str(&format!("\nworkflow {stem:?}: INVALID — {e}\n"));
        }
    }
}

// ────────────────────────── registration ──────────────────────────

/// Build the skill tool registrations around one shared manager.
///
/// Pass the same [`SkillManager`] used for prompt injection so the
/// index in the system prompt, the tools, and the usage telemetry
/// always agree.
///
pub fn skill_registrations(manager: Arc<SkillManager>) -> Vec<ToolRegistration> {
    skill_registrations_with_kms(manager, None)
}

/// Build skill tools with a KMS projection as the preferred read path.
///
/// The filesystem registry remains the fallback while the asynchronous KMS
/// importer catches up or for embedded hosts that do not open a KMS store.
pub fn skill_registrations_with_kms(
    manager: Arc<SkillManager>,
    kms: Option<Arc<KmsSystem>>,
) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SkillListTool {
            manager: manager.clone(),
            kms: kms.clone(),
        }),
        ToolRegistration::from(SkillGetTool {
            manager: manager.clone(),
            kms: kms.clone(),
        }),
        ToolRegistration::from(SkillWorkflowsTool {
            manager: manager.clone(),
            kms: kms.clone(),
        }),
        ToolRegistration::from(SkillSearchTool { manager, kms }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::memory::MemoryStore;
    use agentik_core::tools::ToolFunction as _;

    fn manager_with_two() -> Arc<SkillManager> {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        for (dir, name, desc, tags) in [
            ("a", "gwas-qc", "QC checklist.", "gwas"),
            ("b", "mr-eve", "Mendelian randomization.", "causal"),
        ] {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: {desc}\ntags: [{tags}]\n---\nline1\nline2\nline3\n"
                ),
            )
            .unwrap();
        }
        // Leak the tempdir for the manager's lifetime — the test ends
        // before cleanup matters.
        let state_dir = tmp.keep();
        Arc::new(SkillManager::new(state_dir))
    }

    #[test]
    fn legacy_evolution_tool_is_not_registered_by_default() {
        let registrations = skill_registrations(manager_with_two());
        let names = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["skill_list", "skill_get", "skill_workflows", "skill_search"]
        );
    }

    #[tokio::test]
    async fn list_reports_builtin_and_installed() {
        let tool = SkillListTool {
            manager: manager_with_two(),
            kms: None,
        };
        let out = tool.run(SkillListInput { tier: None }).await.unwrap();
        let text = out.text_content();
        assert!(text.contains("gwas-qc (global)"));
        assert!(text.contains("skill-system (builtin)"));
    }

    #[tokio::test]
    async fn get_paginates_and_reports_continuation() {
        let tool = SkillGetTool {
            manager: manager_with_two(),
            kms: None,
        };
        let out = tool
            .run(SkillGetInput {
                name: "gwas-qc".into(),
                offset: Some(2),
                limit: Some(1),
            })
            .await
            .unwrap();
        let text = out.text_content();
        assert!(text.contains("line2"));
        assert!(text.contains("offset=3"));

        let missing = tool
            .run(SkillGetInput {
                name: "nope".into(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(missing.is_error.is_some_and(|e| e));
    }

    #[tokio::test]
    async fn kms_projection_is_preferred_over_filesystem_registry() {
        let manager = manager_with_two();
        let store = Arc::new(
            agentik_core::memory::TursoKmsStorage::open_in_memory()
                .await
                .unwrap(),
        );
        store
            .import_skill_version(agentik_core::memory::KnowledgeSkillVersionInput {
                skill_name: "kms-only".to_string(),
                title: "KMS-only procedure".to_string(),
                body_md: "Prefer the KMS projection.".to_string(),
                metadata_json: serde_json::json!({
                    "description": "Procedure stored in KMS.",
                    "tags": ["kms"],
                    "tier": "workspace",
                }),
                node_path: "/procedures/kms-only".to_string(),
                source_tier: "workspace".to_string(),
                source_path: None,
                artifacts: vec![agentik_core::memory::KnowledgeSkillArtifactInput {
                    relative_path: "workflow/check.toml".to_string(),
                    content: r#"
description = "Run the KMS-backed check."

[[node]]
id = "check"
kind = "file_to_dataframe"
spec = { path = "/input.csv" }
"#
                    .to_string(),
                }],
            })
            .await
            .unwrap();
        let kms = KmsSystem::new(
            agentik_core::memory::MemoryConfig::new(),
            store,
            Arc::new(agentik_core::memory::EmptyMemorySource),
        )
        .await;

        let listed = SkillListTool {
            manager: Arc::clone(&manager),
            kms: Some(Arc::clone(&kms)),
        }
        .run(SkillListInput { tier: None })
        .await
        .unwrap();
        let listed_text = listed.text_content();
        assert!(listed_text.contains("kms-only (workspace)"));
        assert!(!listed_text.contains("gwas-qc"));
        assert!(!listed_text.contains("skill-system"));

        let workflows = SkillWorkflowsTool {
            manager: Arc::clone(&manager),
            kms: Some(Arc::clone(&kms)),
        }
        .run(SkillWorkflowsInput {
            name: "kms-only".into(),
        })
        .await
        .unwrap();
        assert!(workflows.text_content().contains("KMS-backed check"));

        let searched = SkillSearchTool {
            manager: Arc::clone(&manager),
            kms: Some(Arc::clone(&kms)),
        }
        .run(SkillSearchInput {
            query: "procedure kms".into(),
        })
        .await
        .unwrap();
        assert!(searched.text_content().contains("kms-only"));

        let fetched = SkillGetTool {
            manager: Arc::clone(&manager),
            kms: Some(Arc::clone(&kms)),
        }
        .run(SkillGetInput {
            name: "kms-only".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();
        assert!(
            fetched
                .text_content()
                .contains("Prefer the KMS projection.")
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let evidence = kms.store().list_pending_evidence(100).await.unwrap();
            if evidence
                .iter()
                .any(|item| item.evidence_kind == "search_hit")
                && evidence.iter().any(|item| item.evidence_kind == "get")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "KMS worker never produced skill read evidence"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        kms.shutdown();
    }

    #[tokio::test]
    async fn search_andrs_terms_case_insensitively() {
        let tool = SkillSearchTool {
            manager: manager_with_two(),
            kms: None,
        };
        let out = tool
            .run(SkillSearchInput {
                query: "MR CAUSAL".into(),
            })
            .await
            .unwrap();
        assert!(out.text_content().contains("mr-eve"));
    }
}
