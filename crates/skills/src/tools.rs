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

use crate::format::MAX_BODY_BYTES;
use crate::manager::SkillManager;

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
}

#[async_trait::async_trait]
impl ToolFunction for SkillListTool {
    type Input = SkillListInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let mut entries = self.manager.registry().list();
        if let Some(tier) = input
            .tier
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            entries.retain(|e| e.tier.as_str() == tier);
        }
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
}

#[async_trait::async_trait]
impl ToolFunction for SkillGetTool {
    type Input = SkillGetInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Ok(ToolResult::error("skill_get: 'name' must be non-empty"));
        }
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

        self.manager
            .record_usage(&doc.meta.name, crate::UsageKind::Get);
        let lines: Vec<&str> = doc.body.lines().collect();
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

        let mut out = format!("# {} ({})\n\n", doc.meta.name, doc.tier.as_str());
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
}

#[async_trait::async_trait]
impl ToolFunction for SkillSearchTool {
    type Input = SkillSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
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
}

#[async_trait::async_trait]
impl ToolFunction for SkillWorkflowsTool {
    type Input = SkillWorkflowsInput;

    async fn run(&self, input: SkillWorkflowsInput) -> Result<ToolResult, ToolError> {
        let name = input.name.trim();
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
            match crate::workflow::WorkflowTemplate::load(dir, stem) {
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
        out.push_str("\nInstantiate with skill_run_workflow (skill, workflow, params).");
        Ok(ToolResult::success(out))
    }
}

// ────────────────────────── observation ──────────────────────────

#[tool(
    name = "skill_observe",
    description = "Record one durable, reusable observation for the skill \
        evolution loop: a failure and its fix, a verified recipe, or a caveat \
        where a usual approach breaks. Observations are clustered \
        deterministically by node kind and error signature; a pattern seen \
        three or more times becomes a skill proposal for human review. \
        Record only what future work would otherwise repeat: name the \
        component, state the reusable fix or condition — never run \
        transcripts or one-off facts."
)]
pub struct SkillObserveInput {
    #[desc = "One line a future search would find; name the component, command, or interface."]
    pub summary: String,
    #[desc = "The reusable pattern, fix, or condition. Not a run transcript."]
    pub body: String,
    #[desc = "Observation kind: failure (a fix for an error), recipe (verified how-to), or caveat (where an approach breaks)."]
    pub kind: Option<String>,
    #[desc = "DAG node kind involved, when the observation is anchored to one (e.g. file_to_dataframe). Anchored failures drive auto-distillation."]
    pub node_kind: Option<String>,
    #[desc = "The error text when this is a failure observation. Distillation clusters by its signature."]
    pub error: Option<String>,
}

pub struct SkillObserveTool {
    pub manager: Arc<SkillManager>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillObserveTool {
    type Input = SkillObserveInput;

    async fn run(&self, input: SkillObserveInput) -> Result<ToolResult, ToolError> {
        let summary = input.summary.trim();
        let body = input.body.trim();
        if summary.is_empty() || body.is_empty() {
            return Ok(ToolResult::error(
                "skill_observe: 'summary' and 'body' must be non-empty",
            ));
        }
        let kind = match input.kind.as_deref().map(str::trim) {
            None | Some("") | Some("failure") => crate::ObservationKind::Failure,
            Some("recipe") => crate::ObservationKind::Recipe,
            Some("caveat") => crate::ObservationKind::Caveat,
            Some(other) => {
                return Ok(ToolResult::error(format!(
                    "skill_observe: unknown kind {other:?} \
                     (use failure, recipe, or caveat)"
                )));
            }
        };
        let observation_input = crate::ObservationInput {
            kind,
            source: crate::ObservationSource::Agent,
            summary: summary.to_string(),
            body: body.to_string(),
            node_kind: input
                .node_kind
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            error: input
                .error
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        };
        match self.manager.record_observation(observation_input) {
            Ok(observation) => Ok(ToolResult::success(format!(
                "recorded observation {} ({}). Repeated patterns surface as \
                 skill proposals via `autonomics-skills distill`.",
                observation.id,
                observation.kind_label(),
            ))),
            Err(e) => Ok(ToolResult::error(format!("skill_observe: {e}"))),
        }
    }
}

// ────────────────────────── evolution ──────────────────────────

#[tool(
    name = "skill_evolve",
    description = "Run one skill-evolution cycle now: cluster anchored \
        observations, write skill proposals for repeated patterns, and \
        revise installed auto-skills with new evidence. Deliberately \
        propose-only — the cycle never auto-approves; a human reviews \
        pending proposals. Use after recording several observations, or \
        to check whether accumulated failures have distilled into \
        something reusable."
)]
pub struct SkillEvolveInput {
    #[desc = "Unused placeholder — the cycle takes no parameters. Kept as a \
        named field because tool inputs are field-carrying structs."]
    pub _unused: Option<String>,
}

pub struct SkillEvolveTool {
    pub manager: Arc<SkillManager>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillEvolveTool {
    type Input = SkillEvolveInput;

    async fn run(&self, _input: SkillEvolveInput) -> Result<ToolResult, ToolError> {
        // Propose-only by construction: an agent may trigger the
        // workflow but never lift the review gate — auto-approval is
        // a daemon-level configuration, not a model decision.
        let policy = crate::evolution::EvolutionPolicy::default();
        match crate::evolution::run_evolution_cycle(&self.manager, &policy) {
            Ok(report) => {
                let mut out = format!(
                    "Evolution cycle: {} cluster(s) considered.\n",
                    report.clusters_considered
                );
                for name in &report.proposals_written {
                    if report.updated_existing.contains(name) {
                        out.push_str(&format!("- updated proposal for {name}\n"));
                    } else {
                        out.push_str(&format!("- new proposal {name} (pending review)\n"));
                    }
                }
                for (hash, reason) in &report.skipped {
                    out.push_str(&format!("- skipped {hash}: {reason}\n"));
                }
                if report.proposals_written.is_empty() {
                    out.push_str(
                        "No new patterns. Anchored failure patterns propose at >= 3 \
                         occurrences; record more with skill_observe.",
                    );
                } else {
                    out.push_str(
                        "\nA human reviews pending proposals (autonomics-skills \
                         proposals / approve).",
                    );
                }
                Ok(ToolResult::success(out))
            }
            Err(e) => Ok(ToolResult::error(format!("skill_evolve: {e}"))),
        }
    }
}

// ────────────────────────── registration ──────────────────────────

/// Build the skill tool registrations around one shared manager.
///
/// Pass the same [`SkillManager`] used for prompt injection so the
/// index in the system prompt, the tools, and the usage telemetry
/// always agree.
pub fn skill_registrations(manager: Arc<SkillManager>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SkillListTool {
            manager: manager.clone(),
        }),
        ToolRegistration::from(SkillGetTool {
            manager: manager.clone(),
        }),
        ToolRegistration::from(SkillWorkflowsTool {
            manager: manager.clone(),
        }),
        ToolRegistration::from(SkillSearchTool {
            manager: manager.clone(),
        }),
        ToolRegistration::from(SkillObserveTool {
            manager: manager.clone(),
        }),
        ToolRegistration::from(SkillEvolveTool { manager }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn list_reports_builtin_and_installed() {
        let tool = SkillListTool {
            manager: manager_with_two(),
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
    async fn search_andrs_terms_case_insensitively() {
        let tool = SkillSearchTool {
            manager: manager_with_two(),
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
