//! SKILL.md parsing and validation.
//!
//! A skill is a directory whose `SKILL.md` carries YAML frontmatter:
//!
//! ```yaml
//! ---
//! name: my-skill
//! description: One line on when to use it.
//! tags: [gwas, workflow]
//! ---
//! # Body — operational instructions, read on demand via `skill_get`.
//! ```
//!
//! The contract is deliberately the same shape as the Anthropic
//! `skills` / EvoScientist `EvoSkills` ecosystems so third-party packs
//! install unchanged. Parsing is **lenient** — unknown frontmatter keys
//! are ignored, not rejected — because the registry must keep working
//! when the upstream ecosystem adds fields. Strict validation (name ==
//! directory, description limits) is enforced where it matters: at
//! install time and, later, at proposal time.

use std::path::Path;

use serde::Deserialize;

use crate::error::SkillError;

/// Hard cap on the whole SKILL.md file. Skills are prompt material; a
/// multi-megabyte "skill" is a mistake, not a skill.
pub const MAX_SKILL_MD_BYTES: u64 = 1024 * 1024;

/// Cap on the body slice `skill_get` will return in one response.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// Sanity cap on `description` length at install time — a bound against
/// pathological files, not a style rule. Upstream ecosystems carry
/// >1024-char trigger descriptions; the prompt boundary truncates for
/// display (see [`prompt_safe_description`]).
pub const MAX_DESCRIPTION_CHARS: usize = 4096;

/// Frontmatter fields we consume. Everything else passes through
/// unparsed (see module docs on leniency).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    tags: serde_yaml::Value,
}

/// Parsed skill metadata — the fields the registry, prompt index, and
/// search see. The body is kept out on purpose: entries stay cheap to
/// build on every scan, and the body is only read on `skill_get`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// Split a SKILL.md document into `(frontmatter_yaml, body)`.
///
/// Frontmatter is the block between `---` fences at the very start of
/// the file. When absent the whole document is the body and the caller
/// decides whether that is fatal (registry: yes; lenient readers: no).
pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---")?;
    // Tolerate a single `\r` from CRLF files before the newline.
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n')?;
    let mut lines = rest.split_inclusive('\n');
    let mut consumed = 0usize;
    for line in lines.by_ref() {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            let yaml = &rest[..consumed];
            let body = &rest[consumed + line.len()..];
            return Some((yaml, body));
        }
        consumed += line.len();
    }
    None
}

/// Normalize a frontmatter `tags` value to a list of strings.
///
/// Accepts a YAML sequence (`[a, b]`) or a comma-separated string
/// (`"a, b"`), mirroring the convention the upstream ecosystems use.
fn normalize_tags(value: serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::Null => Vec::new(),
        serde_yaml::Value::Sequence(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(s) => Some(s.trim().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        serde_yaml::Value::String(s) => s
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse one SKILL.md document into metadata plus body.
///
/// Lenient on unknown keys, strict on the two fields we depend on:
/// `name` and `description` must be present, non-empty strings. The
/// description length / angle-bracket rules are checked by
/// [`validate_meta`] at install time, not here — an installed skill with
/// an over-long description still deserves to be listed and readable.
pub fn parse_skill_md(content: &str) -> Result<(SkillMeta, String), SkillError> {
    let (yaml, body) = split_frontmatter(content).ok_or_else(|| {
        SkillError::invalid_frontmatter(
            "<memory>",
            "missing YAML frontmatter: the file must start with a `---` fenced block",
        )
    })?;
    let fm: Frontmatter = serde_yaml::from_str(yaml).map_err(|e| {
        SkillError::invalid_frontmatter("<memory>", format!("invalid frontmatter YAML: {e}"))
    })?;
    if fm.name.trim().is_empty() {
        return Err(SkillError::invalid_frontmatter(
            "<memory>",
            "frontmatter `name` must be a non-empty string",
        ));
    }
    if fm.description.trim().is_empty() {
        return Err(SkillError::invalid_frontmatter(
            "<memory>",
            "frontmatter `description` must be a non-empty string",
        ));
    }
    let meta = SkillMeta {
        name: fm.name.trim().to_string(),
        description: collapse_whitespace(&fm.description),
        tags: normalize_tags(fm.tags),
    };
    Ok((meta, body.trim_start().to_string()))
}

/// Collapse all whitespace runs to single spaces. Multi-line block
/// scalars are common in upstream frontmatter; the registry and prompt
/// index need them as one line.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render a description for model-facing listings (prompt index, tool
/// output): whitespace collapsed, angle brackets stripped (a
/// description cannot smuggle pseudo-XML into the system prompt), and
/// truncated to `max_chars` with an ellipsis.
///
/// This is the sanitization point. Installation is deliberately
/// permissive so upstream ecosystems install unchanged; the trust
/// boundary is wherever text reaches the model.
pub fn prompt_safe_description(description: &str, max_chars: usize) -> String {
    let collapsed = collapse_whitespace(description);
    let filtered: String = collapsed
        .chars()
        .filter(|c| *c != '<' && *c != '>')
        .collect();
    if filtered.chars().count() > max_chars {
        let mut out: String = filtered.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        filtered
    }
}

/// Parse the SKILL.md at `path`, enforcing the file-size cap.
pub fn parse_skill_file(path: &Path) -> Result<(SkillMeta, String), SkillError> {
    let metadata = std::fs::metadata(path).map_err(|e| SkillError::Unreadable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    if metadata.len() > MAX_SKILL_MD_BYTES {
        return Err(SkillError::Unreadable {
            path: path.to_path_buf(),
            reason: format!("SKILL.md exceeds the {} byte cap", MAX_SKILL_MD_BYTES),
        });
    }
    let content = std::fs::read_to_string(path).map_err(|e| SkillError::Unreadable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    parse_skill_md(&content).map_err(|e| match e {
        SkillError::InvalidFrontmatter { reason, .. } => {
            SkillError::invalid_frontmatter(path, reason)
        }
        other => other,
    })
}

/// Validate metadata for installation: naming shape and description
/// length sanity. Deliberately permissive on description *content* —
/// angle brackets and unusual characters are common in upstream
/// ecosystems and are sanitized at the prompt boundary instead (see
/// [`prompt_safe_description`]).
pub fn validate_meta(meta: &SkillMeta) -> Result<(), SkillError> {
    if !is_valid_name(&meta.name) {
        return Err(SkillError::InvalidName(meta.name.clone()));
    }
    if meta.description.len() > MAX_DESCRIPTION_CHARS {
        return Err(SkillError::invalid_frontmatter(
            format!("skill {}", meta.name),
            format!("description must be at most {MAX_DESCRIPTION_CHARS} chars"),
        ));
    }
    Ok(())
}

/// Valid skill names: lowercase kebab-case, 1–64 chars, alphanumerics
/// and `-`, must start and end alphanumeric. Matches the union of the
/// Anthropic-skill and EvoSkills conventions so packs from either
/// ecosystem install unchanged.
pub fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let alphanumeric = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !alphanumeric(bytes[0]) || !alphanumeric(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes.iter().all(|&b| alphanumeric(b) || b == b'-')
}

/// Sanitize an arbitrary label into a valid skill name, or `None` when
/// nothing valid remains. Lowercases, replaces runs of disallowed
/// characters with a single `-`, trims leading/trailing `-`.
pub fn sanitize_name(label: &str) -> Option<String> {
    let mut out = String::with_capacity(label.len());
    let mut pending_dash = false;
    for ch in label.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        let valid = lower.is_ascii_lowercase() || lower.is_ascii_digit();
        if valid {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(lower);
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    let candidate = out.trim_end_matches('-').to_string();
    let candidate = &candidate[..candidate
        .char_indices()
        .nth(64)
        .map(|(i, _)| i)
        .unwrap_or(candidate.len())];
    let candidate = candidate.trim_matches('-');
    (!candidate.is_empty()).then(|| candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\nname: gwas-qc\ndescription: QC checklist for GWAS sumstats.\ntags: [gwas, qc]\n---\n\n# Body\n\nDo the thing.\n";

    #[test]
    fn parses_frontmatter_and_body() {
        let (meta, body) = parse_skill_md(DOC).unwrap();
        assert_eq!(meta.name, "gwas-qc");
        assert_eq!(meta.description, "QC checklist for GWAS sumstats.");
        assert_eq!(meta.tags, vec!["gwas".to_string(), "qc".to_string()]);
        assert!(body.starts_with("# Body"));
    }

    #[test]
    fn accepts_comma_separated_tags() {
        let doc = "---\nname: a\ndescription: d\ntags: \"x, y\"\n---\nbody";
        let (meta, _) = parse_skill_md(doc).unwrap();
        assert_eq!(meta.tags, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn accepts_unknown_keys_and_multiline_descriptions() {
        let doc = "---\nname: a-skill\ndescription: >-\n  folded across\n  two lines\nallowed-tools: read write\nmetadata:\n  version: 2\n---\nbody";
        let (meta, _) = parse_skill_md(doc).unwrap();
        assert_eq!(meta.description, "folded across two lines");
    }

    #[test]
    fn tolerates_crlf_fences() {
        let doc = "---\r\nname: crlf\r\ndescription: d\r\n---\r\nbody";
        let (meta, body) = parse_skill_md(doc).unwrap();
        assert_eq!(meta.name, "crlf");
        assert_eq!(body, "body");
    }

    #[test]
    fn rejects_missing_frontmatter_and_missing_fields() {
        assert!(parse_skill_md("just a body").is_err());
        assert!(parse_skill_md("---\ndescription: d\n---\nb").is_err());
        assert!(parse_skill_md("---\nname: n\n---\nb").is_err());
    }

    #[test]
    fn name_rules() {
        assert!(is_valid_name("a"));
        assert!(is_valid_name("gwas-qc-2"));
        assert!(!is_valid_name("Gwas"));
        assert!(!is_valid_name("-gwas"));
        assert!(!is_valid_name("gwas-"));
        assert!(!is_valid_name("gwas_qc"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name(&"x".repeat(65)));
    }

    #[test]
    fn sanitize_collapses_and_lowercases() {
        assert_eq!(
            sanitize_name("My Cool Skill!").as_deref(),
            Some("my-cool-skill")
        );
        assert_eq!(sanitize_name("  --__--").as_deref(), None);
        assert_eq!(sanitize_name("ÜberSkill").as_deref(), Some("berskill"));
    }

    #[test]
    fn validate_accepts_angle_brackets_but_rejects_long_descriptions() {
        // Angle brackets are ecosystem-legal at ingest; sanitized at
        // the prompt boundary instead.
        let brackets = SkillMeta {
            name: "ok-name".into(),
            description: "use <secret> tags".into(),
            tags: vec![],
        };
        assert!(validate_meta(&brackets).is_ok());
        let long = SkillMeta {
            name: "ok-name".into(),
            description: "x".repeat(4097),
            tags: vec![],
        };
        assert!(validate_meta(&long).is_err());
    }

    #[test]
    fn prompt_safe_description_strips_and_truncates() {
        // Whitespace collapsed, angle brackets removed.
        assert_eq!(
            prompt_safe_description("use <b>bold</b>  tags", 100),
            "use bbold/b tags"
        );
        assert_eq!(prompt_safe_description("short one", 100), "short one");
        let truncated = prompt_safe_description("abcdef", 4);
        assert_eq!(truncated, "abc…");
    }
}
