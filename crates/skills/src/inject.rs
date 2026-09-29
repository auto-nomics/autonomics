//! System-prompt injection: the skill index section.
//!
//! Progressive disclosure in one rule: the prompt carries *which*
//! skills exist and *when* to reach for them; the operational body is
//! fetched on demand via `skill_get`. Keeping the index to one line
//! per skill bounds the prompt cost of a large library.

use crate::registry::SkillEntry;

/// Header of the injected section. Rendered only when at least one
/// skill is visible.
pub const SECTION_HEADER: &str = "## Skills\n";

/// Render the skill index section for a system prompt.
///
/// Returns an empty string when `entries` is empty — an agent with no
/// skills installed gets no section at all, not an empty heading.
pub fn prompt_section(entries: &[SkillEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from(SECTION_HEADER);
    out.push_str(
        "\nReusable skill guides are installed. Each line lists the skill \
         name and when to use it. Before multi-step work, search for \
         coverage with `skill_search`; when one matches, fetch it with \
         `skill_get <name>` and follow its SKILL.md instead of improvising \
         an equivalent procedure.\n\n",
    );
    for entry in entries {
        out.push_str(&format!(
            "- {} — {}\n",
            entry.meta.name,
            crate::format::prompt_safe_description(&entry.meta.description, 240)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::SkillMeta;
    use crate::registry::SkillTier;

    fn entry(name: &str, description: &str) -> SkillEntry {
        SkillEntry::synthetic(
            SkillMeta {
                name: name.to_string(),
                description: description.to_string(),
                tags: vec![],
            },
            SkillTier::Global,
        )
    }

    #[test]
    fn empty_entries_render_nothing() {
        assert_eq!(prompt_section(&[]), "");
    }

    #[test]
    fn entries_render_one_line_each() {
        let section = prompt_section(&[
            entry("gwas-qc", "QC for sumstats."),
            entry("mr-eve", "Mendelian randomization."),
        ]);
        assert!(section.starts_with("## Skills\n"));
        assert!(section.contains("- gwas-qc — QC for sumstats.\n"));
        assert!(section.contains("- mr-eve — Mendelian randomization.\n"));
    }
}
