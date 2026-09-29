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
use crate::registry::{SkillRegistry, SkillTier};

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
    pub registry: Arc<SkillRegistry>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillListTool {
    type Input = SkillListInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let mut entries = self.registry.list();
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
                crate::format::prompt_safe_description(&entry.meta.description, 240,)
            ));
        }
        Ok(ToolResult::success(out))
    }
}

pub struct SkillGetTool {
    pub registry: Arc<SkillRegistry>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillGetTool {
    type Input = SkillGetInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Ok(ToolResult::error("skill_get: 'name' must be non-empty"));
        }
        let Some(doc) = self.registry.get(name).unwrap_or_else(|e| {
            tracing::warn!(skill = name, error = %e, "skill_get: unreadable skill");
            None
        }) else {
            let names: Vec<String> = self
                .registry
                .list()
                .into_iter()
                .map(|e| e.meta.name)
                .collect();
            return Ok(ToolResult::error(format!(
                "skill_get: no skill named {name:?}. Installed: {}",
                names.join(", ")
            )));
        };

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
    pub registry: Arc<SkillRegistry>,
}

#[async_trait::async_trait]
impl ToolFunction for SkillSearchTool {
    type Input = SkillSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entries = self.registry.search(&input.query);
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
                crate::format::prompt_safe_description(&entry.meta.description, 240,)
            ));
        }
        Ok(ToolResult::success(out))
    }
}

// ────────────────────────── registration ──────────────────────────

/// Build the skill tool registrations around one shared registry.
///
/// Pass the same [`SkillRegistry`] used for prompt injection so the
/// index in the system prompt and the tools always agree.
pub fn skill_registrations(registry: Arc<SkillRegistry>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SkillListTool {
            registry: registry.clone(),
        }),
        ToolRegistration::from(SkillGetTool {
            registry: registry.clone(),
        }),
        ToolRegistration::from(SkillSearchTool { registry }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;

    fn registry_with_two() -> Arc<SkillRegistry> {
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
        // Leak the tempdir for the registry's lifetime — the test ends
        // before cleanup matters.
        let root = {
            let leaked = tmp.keep();
            leaked.join("skills")
        };
        Arc::new(SkillRegistry::empty().with_root(SkillTier::Global, root))
    }

    #[tokio::test]
    async fn list_reports_builtin_and_installed() {
        let tool = SkillListTool {
            registry: registry_with_two(),
        };
        let out = tool.run(SkillListInput { tier: None }).await.unwrap();
        let text = out.text_content();
        assert!(text.contains("gwas-qc (global)"));
        assert!(text.contains("skill-system (builtin)"));
    }

    #[tokio::test]
    async fn get_paginates_and_reports_continuation() {
        let tool = SkillGetTool {
            registry: registry_with_two(),
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
            registry: registry_with_two(),
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
