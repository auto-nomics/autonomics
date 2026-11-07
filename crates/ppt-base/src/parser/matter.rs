//! Frontmatter extraction — port of Slidev `matter()`
//! (`packages/parser/src/core.ts:79-110`).
//!
//! Two styles: the classic `---\n…\n---` block, or a leading
//! ```` ```yaml ```` code block. The regexes are ported exactly.

use serde_json::{Map, Value};

/// Extracted frontmatter plus the remaining slide content.
#[derive(Debug, Clone, PartialEq)]
pub struct Matter {
    pub style: FrontmatterStyle,
    /// The raw YAML text (between the markers), verbatim.
    pub frontmatter_raw: Option<String>,
    /// Parsed frontmatter as a JSON object (YAML values coerced like JS).
    pub frontmatter: Map<String, Value>,
    /// Slide content with the frontmatter block removed (NOT trimmed;
    /// callers trim like Slidev does).
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontmatterStyle {
    /// `---\n…\n---` block.
    Frontmatter,
    /// Leading ```` ```yaml ```` code block.
    YamlCodeblock,
}

/// `^---[^\n]*\r?\n([\s\S]*?)---` — anchored headmatter; the capture is
/// lazy so it stops at the first `---` (Slidev's exact behavior, quirks
/// included).
static RE_FRONTMATTER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?s)^---[^\n]*\r?\n(.*?)---").expect("valid regex")
});

/// `^\s*```ya?ml([\s\S]*?)````
static RE_YAML_CODEBLOCK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?s)^\s*```ya?ml(.*?)```").expect("valid regex")
});

pub fn matter(code: &str) -> Matter {
    if let Some(caps) = RE_FRONTMATTER.captures(code) {
        let raw = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let content = code[caps.get(0).unwrap().end()..].to_string();
        let frontmatter = parse_yaml_object(&raw);
        return Matter {
            style: FrontmatterStyle::Frontmatter,
            frontmatter_raw: Some(raw),
            frontmatter,
            content,
        };
    }

    if let Some(caps) = RE_YAML_CODEBLOCK.captures(code) {
        let raw = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let content = code[caps.get(0).unwrap().end()..].to_string();
        let frontmatter = parse_yaml_object(&raw);
        return Matter {
            style: FrontmatterStyle::YamlCodeblock,
            frontmatter_raw: Some(raw),
            frontmatter,
            content,
        };
    }

    Matter {
        style: FrontmatterStyle::Frontmatter,
        frontmatter_raw: None,
        frontmatter: Map::new(),
        content: code.to_string(),
    }
}

/// Parse a YAML document into a JSON object. Non-string keys are coerced
/// to strings (JS `toJSON` behavior). Invalid YAML yields an empty object
/// (Slidev's `YAML.parseDocument` surfaces errors via the document
/// diagnostics; we degrade to `{}` — typed conversion of known fields is
/// tolerant, and patch operations validate YAML explicitly).
pub fn parse_yaml_object(raw: &str) -> Map<String, Value> {
    let value = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "unparseable frontmatter YAML");
            return Map::new();
        }
    };
    match yaml_to_json(&value) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Convert a YAML value to a JSON value, stringifying non-string keys.
pub fn yaml_to_json(v: &serde_yaml_ng::Value) -> Value {
    match v {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(b) => Value::Bool(*b),
        serde_yaml_ng::Value::Number(n) => {
            // Route through serde to get JSON-compatible numbers.
            serde_json::to_value(n).unwrap_or(Value::Null)
        }
        serde_yaml_ng::Value::String(s) => Value::String(s.clone()),
        serde_yaml_ng::Value::Sequence(seq) => {
            Value::Array(seq.iter().map(yaml_to_json).collect())
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml_ng::Value::String(s) => s.clone(),
                    other => match yaml_to_json(other) {
                        Value::String(s) => s,
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".into(),
                        other => other.to_string(),
                    },
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        serde_yaml_ng::Value::Tagged(t) => yaml_to_json(&t.value),
    }
}

/// Serialize a JSON object back to YAML text (used when patching
/// frontmatter values that the line-splice path cannot handle).
pub fn json_to_yaml(v: &Value) -> String {
    serde_yaml_ng::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classic_frontmatter() {
        let m = matter("---\nlayout: cover\ntitle: Hello\n---\n\n# Body\n");
        assert_eq!(m.style, FrontmatterStyle::Frontmatter);
        assert_eq!(m.frontmatter.get("layout"), Some(&json!("cover")));
        assert_eq!(m.frontmatter.get("title"), Some(&json!("Hello")));
        // Matter.content is NOT trimmed (parse_slide_raw trims).
        assert_eq!(m.content, "\n\n# Body\n");
        assert_eq!(m.frontmatter_raw.as_deref(), Some("layout: cover\ntitle: Hello\n"));
    }

    #[test]
    fn yaml_codeblock_style() {
        let m = matter("```yaml\nlayout: cover\n```\n\n# Body\n");
        assert_eq!(m.style, FrontmatterStyle::YamlCodeblock);
        assert_eq!(m.frontmatter.get("layout"), Some(&json!("cover")));
        assert_eq!(m.content, "\n\n# Body\n");
    }

    #[test]
    fn no_frontmatter() {
        let m = matter("# Hello\n\nbody");
        assert!(m.frontmatter.is_empty());
        assert!(m.frontmatter_raw.is_none());
        assert_eq!(m.content, "# Hello\n\nbody");
    }

    #[test]
    fn yaml_types_coerce_to_json() {
        let m = matter("---\ncount: 3\nenabled: true\nratio: 16.5\nnested:\n  a: [1, 2]\n  b: null\n1: numeric-key\n---\nx");
        assert_eq!(m.frontmatter.get("count"), Some(&json!(3)));
        assert_eq!(m.frontmatter.get("enabled"), Some(&json!(true)));
        assert_eq!(m.frontmatter.get("ratio"), Some(&json!(16.5)));
        assert_eq!(m.frontmatter["nested"], json!({"a": [1, 2], "b": null}));
        assert_eq!(m.frontmatter.get("1"), Some(&json!("numeric-key")));
    }

    #[test]
    fn frontmatter_must_be_at_start() {
        let m = matter("text first\n---\nnot: frontmatter\n---\n");
        assert!(m.frontmatter.is_empty());
    }
}
