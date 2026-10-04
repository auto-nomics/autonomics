//! Built-in skills compiled into the binary.
//!
//! The builtin tier is the highest-trust tier: its content ships with
//! the release, so it is readable by every agent without any
//! filesystem prerequisite. Skills are embedded as raw SKILL.md text
//! and parsed through the same [`crate::format`] path as on-disk
//! skills — a malformed embedded skill is an authoring mistake that
//! surfaces as a warning and a skipped entry, never a startup failure.

use crate::format::SkillMeta;

/// One embedded skill. `markdown` is the full SKILL.md source.
pub struct BuiltinSkill {
    pub name: &'static str,
    pub markdown: &'static str,
}

/// The builtin skill table. Order is irrelevant; the registry keys by
/// parsed name.
pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[BuiltinSkill {
    name: "skill-system",
    markdown: include_str!("../assets/builtin/skill-system/SKILL.md"),
}];

/// A parsed builtin entry: metadata plus the post-frontmatter body.
#[derive(Debug, Clone)]
pub struct BuiltinEntry {
    pub meta: SkillMeta,
    pub body: String,
}

/// Parse the builtin table into entries.
///
/// Invalid entries are skipped with a warning — see module docs.
pub fn builtin_entries() -> Vec<BuiltinEntry> {
    BUILTIN_SKILLS
        .iter()
        .filter_map(
            |skill| match crate::format::parse_skill_md(skill.markdown) {
                Ok((meta, body)) => Some(BuiltinEntry { meta, body }),
                Err(e) => {
                    tracing::warn!(
                        skill = skill.name,
                        error = %e,
                        "skipping malformed builtin skill"
                    );
                    None
                }
            },
        )
        .collect()
}
