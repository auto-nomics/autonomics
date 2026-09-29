//! Skill installation: local paths and git sources, with a provenance
//! sidecar per tier.
//!
//! Sources:
//! - a local directory that is itself a skill, or a parent containing
//!   one or more skill directories (one or two levels deep)
//! - `https://github.com/owner/repo`, `.../tree/<ref>/<path>`, or the
//!   `owner/repo@<path>` shorthand — resolved via a shallow clone
//!
//! Installation normalizes each skill's directory to its frontmatter
//! `name`, validates metadata, and records provenance in
//! `<tier>/.installed.toml` so later tooling can answer "where did
//! this come from and has upstream moved".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::SkillError;
use crate::format::{parse_skill_file, sanitize_name, validate_meta};

/// Per-tier provenance sidecar filename, written inside the tier root.
pub const MANIFEST_FILENAME: &str = ".installed.toml";

/// One provenance record for an installed skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    /// The user-facing source string (URL, shorthand, or path).
    pub source: String,
    /// Upstream commit at install time, when the source is git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Unix seconds.
    pub installed_at: i64,
}

/// Result of one install call.
#[derive(Debug, Default)]
pub struct InstallOutcome {
    pub installed: Vec<InstalledSkill>,
    pub failed: Vec<(String, String)>,
}

impl InstallOutcome {
    pub fn success(&self) -> bool {
        !self.installed.is_empty()
    }
}

#[derive(Debug)]
pub struct InstalledSkill {
    pub name: String,
    pub path: PathBuf,
}

/// A parsed git source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// `owner/repo`.
    pub repo: String,
    /// Branch / tag / commit, if the URL carried one.
    pub git_ref: Option<String>,
    /// In-repo subpath to the skill directory.
    pub path: Option<String>,
}

/// Parse a git source: full GitHub URLs or `owner/repo@path` shorthand.
pub fn parse_git_source(source: &str) -> Result<GitSource, SkillError> {
    let shorthand = source.contains('@')
        && !source.contains("://")
        && source.split('@').next().is_some_and(|left| {
            let parts: Vec<&str> = left.split('/').collect();
            parts.len() == 2 && parts.iter().all(|p| !p.is_empty())
        });
    if shorthand {
        let (repo, path) = source.split_once('@').expect("checked above");
        return Ok(GitSource {
            repo: repo.trim().to_string(),
            git_ref: None,
            path: Some(path.trim().to_string()),
        });
    }

    let cleaned = source
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut parts = cleaned.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || owner.starts_with('.') {
        return Err(SkillError::BadSource(source.to_string()));
    }
    let full = format!("{owner}/{repo}");
    match parts.next() {
        None => Ok(GitSource {
            repo: full,
            git_ref: None,
            path: None,
        }),
        Some("tree") => {
            let git_ref = parts.next().map(str::to_string);
            let path = parts.collect::<Vec<_>>().join("/");
            Ok(GitSource {
                repo: full,
                git_ref,
                path: (!path.is_empty()).then_some(path),
            })
        }
        Some(_) => Err(SkillError::BadSource(source.to_string())),
    }
}

/// Install skills from a local directory into `dest_root`.
///
/// `source` may be a skill directory itself or a parent holding skill
/// directories up to two levels deep (a pack). Each skill's
/// destination directory is the frontmatter `name`; an existing
/// directory of the same name is replaced. `.git` subtrees are never
/// copied.
pub fn install_from_local(
    source: &Path,
    dest_root: &Path,
    provenance: Option<&InstallRecord>,
) -> Result<InstallOutcome, SkillError> {
    if !source.is_dir() {
        return Err(SkillError::BadSource(source.display().to_string()));
    }
    // The source may itself be a skill directory…
    let candidates = if source.join("SKILL.md").is_file() {
        vec![source.to_path_buf()]
    } else {
        // …or a parent holding skills one or two levels down (a pack).
        find_skill_dirs(source)
    };
    if candidates.is_empty() {
        return Err(SkillError::MissingSkillMd(source.to_path_buf()));
    }
    let mut outcome = InstallOutcome::default();
    for dir in candidates {
        match install_one(&dir, dest_root, provenance) {
            Ok(installed) => outcome.installed.push(installed),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "skill install failed");
                outcome
                    .failed
                    .push((dir.display().to_string(), e.to_string()));
            }
        }
    }
    Ok(outcome)
}

/// Install every skill found in a git source into `dest_root`.
///
/// Shallow-clones into a temp directory, records the cloned HEAD as
/// provenance, and reuses the local installer on the resolved path.
pub fn install_from_git(source: &str, dest_root: &Path) -> Result<InstallOutcome, SkillError> {
    let git = parse_git_source(source)?;
    let tmp = Scratch::new()?;
    let clone_dir = tmp.path().join("repo");
    let url = format!("https://github.com/{}.git", git.repo);
    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--depth").arg("1");
    if let Some(git_ref) = &git.git_ref {
        cmd.arg("--branch").arg(git_ref);
    }
    cmd.arg(&url).arg(&clone_dir);
    let output = cmd
        .output()
        .map_err(|e| SkillError::Git(format!("cannot run git clone: {e}")))?;
    if !output.status.success() {
        return Err(SkillError::Git(format!(
            "clone of {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let commit = git_rev_parse(&clone_dir);
    let skill_root = match &git.path {
        Some(path) => clone_dir.join(path),
        None => clone_dir.clone(),
    };
    let record = InstallRecord {
        source: source.to_string(),
        commit,
        installed_at: unix_now(),
    };
    // If the explicit path is not itself a skill directory, fall back
    // to scanning the whole clone — packs live at repo roots.
    if skill_root.is_dir() && skill_root.join("SKILL.md").is_file() {
        let mut outcome = install_from_local(&skill_root, dest_root, Some(&record))?;
        if !outcome.success() {
            let mut fallback = install_from_local(&clone_dir, dest_root, Some(&record))?;
            outcome.installed.append(&mut fallback.installed);
            outcome.failed.append(&mut fallback.failed);
        }
        Ok(outcome)
    } else {
        install_from_local(&clone_dir, dest_root, Some(&record))
    }
}

/// Uninstall by name from the first tier root (checked in the given
/// order) that holds it. Returns the removed directory.
pub fn uninstall(name: &str, roots: &[PathBuf]) -> Result<PathBuf, SkillError> {
    let clean = sanitize_name(name).ok_or_else(|| SkillError::InvalidName(name.to_string()))?;
    for root in roots {
        let target = root.join(&clean);
        if target.join("SKILL.md").is_file() {
            std::fs::remove_dir_all(&target)?;
            remove_manifest_entry(root, &clean);
            return Ok(target);
        }
        // The directory name may differ from the frontmatter name for
        // hand-installed skills; resolve by content before giving up.
        if root.is_dir() {
            if let Ok(read_dir) = std::fs::read_dir(root) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    if let Ok((meta, _)) = parse_skill_file(&path.join("SKILL.md")) {
                        if meta.name == clean {
                            std::fs::remove_dir_all(&path)?;
                            remove_manifest_entry(
                                root,
                                path.file_name().and_then(|n| n.to_str()).unwrap_or(&clean),
                            );
                            return Ok(path);
                        }
                    }
                }
            }
        }
    }
    Err(SkillError::NotFound(name.to_string()))
}

// ────────────────────────── internals ──────────────────────────

/// Install one validated skill directory under `dest_root`.
fn install_one(
    dir: &Path,
    dest_root: &Path,
    provenance: Option<&InstallRecord>,
) -> Result<InstalledSkill, SkillError> {
    let (meta, _body) = parse_skill_file(&dir.join("SKILL.md"))?;
    validate_meta(&meta)?;
    let name = sanitize_name(&meta.name)
        .filter(|n| n == &meta.name)
        .ok_or_else(|| SkillError::InvalidName(meta.name.clone()))?;
    std::fs::create_dir_all(dest_root)?;
    let target = dest_root.join(&name);
    let resolved = target.canonicalize().unwrap_or_else(|_| target.clone());
    let root_canonical = dest_root
        .canonicalize()
        .unwrap_or_else(|_| dest_root.to_path_buf());
    if !resolved.starts_with(&root_canonical) {
        return Err(SkillError::InvalidName(meta.name.clone()));
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    copy_tree(dir, &target)?;
    if let Some(record) = provenance {
        record_manifest_entry(dest_root, &name, record)?;
    }
    Ok(InstalledSkill { name, path: target })
}

/// Find skill directories under `root`: direct children with SKILL.md,
/// plus grandchildren inside non-skill child directories (packs one
/// level deep, matching the layout both upstream ecosystems use).
fn find_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return found;
    };
    let mut children: Vec<PathBuf> = read_dir.flatten().map(|e| e.path()).collect();
    children.sort();
    for child in children {
        let name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || !child.is_dir() {
            continue;
        }
        if child.join("SKILL.md").is_file() {
            found.push(child);
        } else if let Ok(read_dir) = std::fs::read_dir(&child) {
            for grandchild in read_dir.flatten().map(|e| e.path()) {
                if grandchild.is_dir()
                    && !grandchild
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .starts_with('.')
                    && grandchild.join("SKILL.md").is_file()
                {
                    found.push(grandchild);
                }
            }
        }
    }
    found
}

/// Recursive copy that skips `.git` directories anywhere in the tree.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        if path.is_dir() {
            if name_str == ".git" {
                continue;
            }
            copy_tree(&path, &dst.join(&name))?;
        } else {
            std::fs::copy(&path, dst.join(&name))?;
        }
    }
    Ok(())
}

/// Records for one tier root, keyed by skill name. A corrupt sidecar
/// reads as empty — provenance must never block installation.
pub fn installed_records(root: &Path) -> BTreeMap<String, InstallRecord> {
    let path = root.join(MANIFEST_FILENAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    match toml::from_str(&text) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "malformed install manifest");
            BTreeMap::new()
        }
    }
}

fn write_records(root: &Path, records: &BTreeMap<String, InstallRecord>) -> Result<(), SkillError> {
    let path = root.join(MANIFEST_FILENAME);
    let text =
        toml::to_string_pretty(records).map_err(|_| SkillError::BadManifest(path.clone()))?;
    let tmp_path = root.join(format!(".{}.{}.tmp", MANIFEST_FILENAME, std::process::id()));
    // Stage + rename so a crash mid-write cannot corrupt the sidecar.
    std::fs::write(&tmp_path, text)?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

fn record_manifest_entry(
    root: &Path,
    name: &str,
    record: &InstallRecord,
) -> Result<(), SkillError> {
    let mut records = installed_records(root);
    records.insert(name.to_string(), record.clone());
    write_records(root, &records)
}

fn remove_manifest_entry(root: &Path, name: &str) {
    let mut records = installed_records(root);
    if records.remove(name).is_some() {
        if let Err(e) = write_records(root, &records) {
            tracing::warn!(path = %root.display(), error = %e, "cannot update install manifest");
        }
    }
}

fn git_rev_parse(dir: &Path) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A self-cleaning scratch directory — `std::env::temp_dir` + pid +
/// counter, so the release binary does not depend on `tempfile` for a
/// clone scratch space we always remove ourselves.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, SkillError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("autonomics-skills-{}-{}", std::process::id(), seq));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pack(root: &Path) {
        // root/pack/ holds two skills one level down.
        for (dir, name, desc) in [
            ("alpha", "alpha-skill", "First skill."),
            ("beta", "beta-skill", "Second skill."),
        ] {
            let d = root.join("pack").join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n"),
            )
            .unwrap();
            std::fs::create_dir_all(d.join(".git")).unwrap();
            std::fs::write(d.join(".git").join("HEAD"), "ref").unwrap();
            std::fs::write(d.join("script.sh"), "#!/bin/sh\necho hi\n").unwrap();
        }
    }

    #[test]
    fn parses_git_sources() {
        assert_eq!(
            parse_git_source("https://github.com/owner/repo").unwrap(),
            GitSource {
                repo: "owner/repo".into(),
                git_ref: None,
                path: None
            }
        );
        assert_eq!(
            parse_git_source("github.com/owner/repo/tree/main/skills/foo").unwrap(),
            GitSource {
                repo: "owner/repo".into(),
                git_ref: Some("main".into()),
                path: Some("skills/foo".into())
            }
        );
        assert_eq!(
            parse_git_source("owner/repo@skills/foo").unwrap(),
            GitSource {
                repo: "owner/repo".into(),
                git_ref: None,
                path: Some("skills/foo".into())
            }
        );
        assert!(parse_git_source("./local/path").is_err());
        assert!(parse_git_source("not a url").is_err());
    }

    #[test]
    fn installs_pack_normalizing_names_and_ignoring_git() {
        let src = Scratch(tempfile::tempdir().unwrap().keep());
        let dest = Scratch(tempfile::tempdir().unwrap().keep());
        make_pack(&src.0);

        let record = InstallRecord {
            source: "local-test".into(),
            commit: None,
            installed_at: 123,
        };
        let outcome = install_from_local(&src.0.join("pack"), &dest.0, Some(&record)).unwrap();
        assert_eq!(outcome.installed.len(), 2);
        assert!(outcome.success());

        let alpha = dest.0.join("alpha-skill");
        assert!(alpha.join("SKILL.md").is_file());
        assert!(alpha.join("script.sh").is_file());
        assert!(!alpha.join(".git").exists());

        let records = installed_records(&dest.0);
        assert_eq!(records.len(), 2);
        assert_eq!(records["alpha-skill"].source, "local-test");

        // Re-install replaces cleanly.
        install_from_local(&src.0.join("pack"), &dest.0, Some(&record)).unwrap();
        assert!(alpha.join("SKILL.md").is_file());
    }

    #[test]
    fn installs_single_skill_directory_directly() {
        // `install <skill-dir>` (not a pack parent) must install the
        // directory itself.
        let src = Scratch(tempfile::tempdir().unwrap().keep());
        let dest = Scratch(tempfile::tempdir().unwrap().keep());
        let dir = src.0.join("my-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: d\n---\nb\n",
        )
        .unwrap();

        let outcome = install_from_local(&dir, &dest.0, None).unwrap();
        assert_eq!(outcome.installed.len(), 1);
        assert!(dest.0.join("my-skill").join("SKILL.md").is_file());
    }

    #[test]
    fn install_rejects_mismatched_or_invalid_names() {
        let src = Scratch(tempfile::tempdir().unwrap().keep());
        let dest = Scratch(tempfile::tempdir().unwrap().keep());
        let d = src.0.join("one");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\nname: Bad_Name\ndescription: d\n---\nb\n",
        )
        .unwrap();
        let outcome = install_from_local(&src.0, &dest.0, None).unwrap();
        assert!(!outcome.success());
        assert_eq!(outcome.failed.len(), 1);
    }

    #[test]
    fn uninstall_removes_by_name_and_updates_manifest() {
        let src = Scratch(tempfile::tempdir().unwrap().keep());
        let dest = Scratch(tempfile::tempdir().unwrap().keep());
        make_pack(&src.0);
        let record = InstallRecord {
            source: "local-test".into(),
            commit: None,
            installed_at: 123,
        };
        install_from_local(&src.0.join("pack"), &dest.0, Some(&record)).unwrap();

        let removed = uninstall("beta-skill", std::slice::from_ref(&dest.0)).unwrap();
        assert!(removed.ends_with("beta-skill"));
        assert!(!dest.0.join("beta-skill").exists());
        assert!(dest.0.join("alpha-skill").exists());
        assert!(!installed_records(&dest.0).contains_key("beta-skill"));

        assert!(uninstall("not-installed", std::slice::from_ref(&dest.0)).is_err());
    }

    #[test]
    fn uninstall_resolves_hand_installed_dir_by_frontmatter_name() {
        let dest = Scratch(tempfile::tempdir().unwrap().keep());
        let d = dest.0.join("odd-dir-name");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\nname: real-name\ndescription: d\n---\nb\n",
        )
        .unwrap();
        let removed = uninstall("real-name", std::slice::from_ref(&dest.0)).unwrap();
        assert!(removed.ends_with("odd-dir-name"));
    }
}
