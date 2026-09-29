//! Tiered skill registry: scan, shadow-dedupe, look up.
//!
//! Tiers, lowest → highest priority:
//!
//! 1. `builtin`   — compiled into the binary ([`crate::builtin`])
//! 2. `global`    — `<state_dir>/skills` (default `~/.autonomics/skills`)
//! 3. `workspace` — an optional extra root supplied by the caller
//!
//! A higher tier **shadows** a lower tier skill with the same parsed
//! name. Deduplication keys on the frontmatter `name`, not the
//! directory name, so a directory renamed by hand cannot smuggle in a
//! duplicate.
//!
//! There is deliberately **no cache**: a scan reads directory entries
//! plus the frontmatter of each skill — microseconds for realistic
//! library sizes — and always-fresh means an installed skill is
//! visible to the next agent build without any invalidation wiring.

use std::path::{Path, PathBuf};

use crate::builtin::{BuiltinEntry, builtin_entries};
use crate::error::SkillError;
use crate::format::{SkillMeta, parse_skill_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillTier {
    Builtin,
    Global,
    Workspace,
}

impl SkillTier {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillTier::Builtin => "builtin",
            SkillTier::Global => "global",
            SkillTier::Workspace => "workspace",
        }
    }
}

/// Where a listed skill's content lives.
#[derive(Debug, Clone)]
enum SkillOrigin {
    /// Full markdown held in memory (builtin tier).
    Embedded { body: String },
    /// Skill directory; `SKILL.md` read on demand.
    Dir(PathBuf),
}

/// One skill as seen by listing / prompt injection / search. The body
/// is intentionally absent — it is fetched only via
/// [`SkillRegistry::get`].
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub meta: SkillMeta,
    pub tier: SkillTier,
    /// The skill bundles instantiable workflow templates
    /// (`workflow/*.toml`).
    pub has_workflows: bool,
    origin: SkillOrigin,
}

impl SkillEntry {
    pub fn name(&self) -> &str {
        &self.meta.name
    }

    /// The on-disk skill directory, when the skill comes from a tier.
    pub fn dir(&self) -> Option<&Path> {
        match &self.origin {
            SkillOrigin::Dir(p) => Some(p.as_path()),
            SkillOrigin::Embedded { .. } => None,
        }
    }

    /// An entry with no backing content, for callers that only need
    /// the listing shape (e.g. rendering a prompt section in tests).
    #[cfg(test)]
    pub(crate) fn synthetic(meta: SkillMeta, tier: SkillTier) -> Self {
        Self {
            meta,
            tier,
            has_workflows: false,
            origin: SkillOrigin::Embedded {
                body: String::new(),
            },
        }
    }
}

/// A skill document returned by [`SkillRegistry::get`]: metadata plus
/// the post-frontmatter body, plus the directory-backed extras
/// (workflow templates, eval files) when the skill comes from a tier.
#[derive(Debug, Clone)]
pub struct SkillDocument {
    pub meta: SkillMeta,
    pub tier: SkillTier,
    pub body: String,
    /// On-disk skill directory for tier-backed skills; `None` for
    /// builtin embedded skills (which carry no workflows today).
    pub dir: Option<PathBuf>,
    /// Workflow template stems under `workflow/`, sorted.
    pub workflows: Vec<String>,
    /// Eval file stems under `evals/`, sorted.
    pub evals: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    builtin: Vec<BuiltinEntry>,
    /// Filesystem roots in ascending priority order; higher entries
    /// shadow lower ones on name collisions.
    roots: Vec<(SkillTier, PathBuf)>,
}

impl SkillRegistry {
    /// An empty registry: builtin tier only, no filesystem roots.
    pub fn empty() -> Self {
        Self {
            builtin: builtin_entries(),
            roots: Vec::new(),
        }
    }

    /// The standard layout: builtin tier plus the global tier at
    /// `<state_dir>/skills`.
    pub fn standard(state_dir: &Path) -> Self {
        Self::empty().with_root(SkillTier::Global, state_dir.join("skills"))
    }

    /// Add a filesystem root at the given tier. Later calls win on
    /// collisions, and a `Workspace` root added after `Global`
    /// naturally shadows it.
    #[must_use]
    pub fn with_root(mut self, tier: SkillTier, root: impl Into<PathBuf>) -> Self {
        self.roots.push((tier, root.into()));
        self
    }

    /// List every visible skill, deduplicated by name (highest tier
    /// wins), sorted by name.
    pub fn list(&self) -> Vec<SkillEntry> {
        let mut by_name: std::collections::BTreeMap<String, SkillEntry> =
            std::collections::BTreeMap::new();
        let mut insert = |entry: SkillEntry| {
            by_name.insert(entry.meta.name.clone(), entry);
        };

        for e in &self.builtin {
            insert(SkillEntry {
                meta: e.meta.clone(),
                tier: SkillTier::Builtin,
                has_workflows: false,
                origin: SkillOrigin::Embedded {
                    body: e.body.clone(),
                },
            });
        }
        for (tier, root) in &self.roots {
            for entry in scan_root(*tier, root) {
                insert(entry);
            }
        }
        by_name.into_values().collect()
    }

    /// Fetch one skill document by exact name.
    pub fn get(&self, name: &str) -> Result<Option<SkillDocument>, SkillError> {
        let Some(entry) = self.list().into_iter().find(|e| e.meta.name == name) else {
            return Ok(None);
        };
        let (body, dir) = match &entry.origin {
            SkillOrigin::Embedded { body } => (body.clone(), None),
            SkillOrigin::Dir(dir) => (
                parse_skill_file(&dir.join("SKILL.md"))?.1,
                Some(dir.clone()),
            ),
        };
        let (workflows, evals) = match &dir {
            Some(dir) => (
                crate::workflow::WorkflowTemplate::stems_in_dir(dir),
                crate::eval::eval_stems_in_dir(dir),
            ),
            None => (Vec::new(), Vec::new()),
        };
        Ok(Some(SkillDocument {
            meta: entry.meta,
            tier: entry.tier,
            body,
            dir,
            workflows,
            evals,
        }))
    }

    /// Case-insensitive substring search over name, tags, and
    /// description. Every term must match somewhere.
    pub fn search(&self, query: &str) -> Vec<SkillEntry> {
        let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        if terms.is_empty() {
            return self.list();
        }
        self.list()
            .into_iter()
            .filter(|e| {
                let name = e.meta.name.to_lowercase();
                let description = e.meta.description.to_lowercase();
                let tags: Vec<String> = e.meta.tags.iter().map(|t| t.to_lowercase()).collect();
                terms.iter().all(|term| {
                    name.contains(term.as_str())
                        || description.contains(term.as_str())
                        || tags.iter().any(|t| t.contains(term.as_str()))
                })
            })
            .collect()
    }

    /// Filesystem roots in tier order (for install/uninstall callers).
    pub fn roots(&self) -> &[(SkillTier, PathBuf)] {
        &self.roots
    }
}

/// Scan one tier root for skill directories (a directory containing
/// `SKILL.md`). Hidden directories are skipped — sidecars such as
/// `.installed.toml` live in the root but never parse as skills.
fn scan_root(tier: SkillTier, root: &Path) -> Vec<SkillEntry> {
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        match parse_skill_file(&skill_md) {
            Ok((meta, _body)) => out.push(SkillEntry {
                meta,
                tier,
                has_workflows: path.join(crate::workflow::WORKFLOW_DIR).is_dir(),
                origin: SkillOrigin::Dir(path),
            }),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping invalid skill");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, dir: &str, name: &str, description: &str) {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nbody of {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn builtin_tier_is_present() {
        let reg = SkillRegistry::empty();
        let names: Vec<String> = reg.list().into_iter().map(|e| e.meta.name).collect();
        assert!(names.contains(&"skill-system".to_string()));
        let doc = reg.get("skill-system").unwrap().unwrap();
        assert!(doc.body.contains("# Using skills"));
        assert_eq!(doc.tier, SkillTier::Builtin);
    }

    #[test]
    fn scans_global_root_and_ignores_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        write_skill(&root, "gwas-qc", "gwas-qc", "QC for sumstats.");
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join(".installed.toml"), "not a dir").unwrap();

        let reg = SkillRegistry::empty().with_root(SkillTier::Global, &root);
        let list = reg.list();
        assert_eq!(list.len(), 2); // builtin + gwas-qc
        let doc = reg.get("gwas-qc").unwrap().unwrap();
        assert_eq!(doc.meta.description, "QC for sumstats.");
        assert_eq!(doc.tier, SkillTier::Global);
        assert!(reg.get("nope").unwrap().is_none());
    }

    #[test]
    fn workspace_shadows_global_by_parsed_name() {
        let global = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // Different *directory* names, same frontmatter name: dedupe
        // must key on the parsed name.
        write_skill(global.path(), "old-dir", "gwas-qc", "old description");
        write_skill(workspace.path(), "renamed", "gwas-qc", "new description");

        let reg = SkillRegistry::empty()
            .with_root(SkillTier::Global, global.path())
            .with_root(SkillTier::Workspace, workspace.path());
        let list = reg.list();
        assert_eq!(list.iter().filter(|e| e.name() == "gwas-qc").count(), 1);
        let doc = reg.get("gwas-qc").unwrap().unwrap();
        assert_eq!(doc.meta.description, "new description");
        assert_eq!(doc.tier, SkillTier::Workspace);
    }

    #[test]
    fn invalid_skills_are_skipped_with_others_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        write_skill(&root, "good", "good-skill", "fine");
        let broken = root.join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("SKILL.md"), "no frontmatter at all").unwrap();

        let reg = SkillRegistry::empty().with_root(SkillTier::Global, &root);
        let names: Vec<String> = reg.list().into_iter().map(|e| e.meta.name).collect();
        assert!(names.contains(&"good-skill".to_string()));
        assert!(!names.contains(&"broken".to_string()));
    }

    #[test]
    fn search_matches_name_tag_and_description() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        write_skill(&root, "a", "ldsc-refine", "Segregation tuning.");
        let b = root.join("b");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            b.join("SKILL.md"),
            "---\nname: mr-eve\ndescription: Mendelian randomization.\ntags: [causal]\n---\nx\n",
        )
        .unwrap();
        let reg = SkillRegistry::empty().with_root(SkillTier::Global, &root);

        assert_eq!(reg.search("ldsc").len(), 1);
        assert_eq!(reg.search("CAUSAL").len(), 1);
        assert_eq!(reg.search("segregation").len(), 1);
        // Multi-term: both must hit.
        assert_eq!(reg.search("mendelian randomization").len(), 1);
        assert!(reg.search("nothing-matches").is_empty());
    }
}
