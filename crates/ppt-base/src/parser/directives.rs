//! Click-directive extraction from slide content.
//!
//! Our declarative subset supports two authoring styles that work in
//! plain markdown (raw HTML passthrough), mirroring Slidev's Vue
//! directives:
//!
//! - component form: `<v-click>` … `</v-click>`, `<v-click at="3">`,
//!   `<v-click at="[2, '+2']">`
//! - attribute form: `<div v-click>`, `<span v-click at="+1">…`
//!
//! Directives are collected in document order (code blocks stripped);
//! the resulting `Vec<ClickDirective>` feeds
//! [`ppt_types::compute_plan`](ppt_types::compute_plan). The frontend
//! performs the same scan over the rendered DOM and matches elements by
//! order, so both sides must agree on the pattern set.

use ppt_types::clicks::ClickDirective;
use serde_json::Value;

static RE_COMPONENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"<v-click\b([^>]*?)/?>"#).expect("valid regex")
});

static RE_ATTR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    // No lookahead in the `regex` crate: the trailing delimiter
    // (`[\s/>]`) or end-of-string is consumed as part of the match; we
    // only need the match position, not its extent.
    regex::Regex::new(r#"\sv-click(?:\s*=\s*("[^"]*"|'[^']*'|[^\s>]+))?(?:[\s/>]|$)"#)
        .expect("valid regex")
});

/// `at="…"` inside a tag opening.
static RE_AT_ATTR: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"\bat\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))"#).expect("valid regex"));

/// Collect click directives in document order (0-based `order` fields).
pub fn extract_click_directives(content: &str) -> Vec<ClickDirective> {
    let stripped = crate::parser::extract::strip_code_blocks(content);
    let mut hits: Vec<(usize, Value)> = Vec::new();

    for caps in RE_COMPONENT.captures_iter(&stripped) {
        let m = caps.get(0).unwrap();
        let attrs = caps.get(1).map(|c| c.as_str()).unwrap_or_default();
        let at = parse_at_attr(attrs);
        hits.push((m.start(), at));
    }

    for caps in RE_ATTR.captures_iter(&stripped) {
        let m = caps.get(0).unwrap();
        let value = caps
            .get(1)
            .map(|c| c.as_str())
            .map(unescape_attr_value)
            .unwrap_or_default();
        hits.push((m.start(), attr_to_value(&value)));
    }

    hits.sort_by_key(|(pos, _)| *pos);
    hits.dedup_by(|a, b| {
        // An attribute hit inside a component opening tag: the attr regex
        // requires preceding whitespace, and `<v-click` is preceded by
        // `<`, so overlaps are impossible — but adjacent same-position
        // duplicates are still filtered defensively.
        a.0 == b.0
    });

    hits.into_iter()
        .enumerate()
        .map(|(order, (_, at))| ClickDirective::new(order, at))
        .collect()
}

fn parse_at_attr(attrs: &str) -> Value {
    match RE_AT_ATTR.captures(attrs) {
        Some(caps) => {
            let raw = caps
                .get(1)
                .map(|c| c.as_str())
                .map(unescape_attr_value)
                .unwrap_or_default();
            attr_to_value(&raw)
        }
        None => Value::Null,
    }
}

fn unescape_attr_value(raw: &str) -> String {
    let raw = raw.trim();
    if (raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2)
        || (raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2)
    {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}

/// Attribute value → JSON `at` value: bare/`true` → null (relative +1);
/// otherwise the string form (normalization happens in `compute_plan`).
fn attr_to_value(v: &str) -> Value {
    if v.is_empty() || v == "true" {
        Value::Null
    } else if v == "false" {
        Value::Bool(false)
    } else {
        Value::String(v.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppt_types::clicks::{compute_plan, SingleAt};

    #[test]
    fn component_form_in_order() {
        let content = "intro\n\n<v-click>\n\n# One\n\n</v-click>\n\n<v-click at=\"3\">\n\nTwo\n\n</v-click>\n\n<v-click at=\"[5, '+2']\">Three</v-click>";
        let d = extract_click_directives(content);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].at, Value::Null);
        assert_eq!(d[1].at, Value::String("3".into()));
        assert_eq!(d[2].at, Value::String("[5, '+2']".into()));
        assert_eq!(d[2].order, 2);
    }

    #[test]
    fn attribute_form() {
        // Slidev semantics: the directive VALUE carries the at
        // (`v-click="+2"`); a bare `v-click` attribute is relative +1.
        let content = "<div v-click>first</div>\n\n<span v-click='+2'>second</span>\n\n<div v-click=false>skipped</div>";
        let d = extract_click_directives(content);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].at, Value::Null);
        assert_eq!(d[1].at, Value::String("+2".into()));
        assert_eq!(d[2].at, Value::Bool(false));
    }

    #[test]
    fn mixed_forms_interleave_by_position() {
        let content = "<v-click>A</v-click>\n\ntext <b v-click>B</b> more <v-click>C</v-click>";
        let d = extract_click_directives(content);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].order, 0);
        assert_eq!(d[1].order, 1);
        assert_eq!(d[2].order, 2);
    }

    #[test]
    fn code_blocks_do_not_count() {
        let content = "```html\n<v-click>inside code</v-click>\n```\n\n<v-click>real</v-click>";
        let d = extract_click_directives(content);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn plan_integration() {
        let content = "<v-click>1</v-click>\n\n<v-click at=\"3\">2</v-click>\n\n<v-click>3</v-click>";
        let d = extract_click_directives(content);
        let plan = compute_plan(&d, None, 0);
        // total = max over element maxima = max(1, 3, 2) = 3
        assert_eq!(plan.total, 3);
        assert_eq!(plan.elements[0].start, 1);
        assert_eq!(plan.elements[1].start, 3);
        // offset after directive 0 is 1; directive 2 is relative → 1+1 = 2
        assert_eq!(plan.elements[2].start, 2);
        assert_eq!(
            plan.elements[0],
            ppt_types::ElementClickState {
                order: 0,
                start: 1,
                end: None,
                max: 1,
                delta: 1
            }
        );
        assert_eq!(SingleAt::Absolute(1), SingleAt::Absolute(1));
    }

    #[test]
    fn self_closing_and_bare_names_not_double_counted() {
        // `<v-click/>` self-closing counts once; `v-click` as substring of
        // other words does not count.
        let content = "<v-click/> ok\nv-clicks everywhere\nclicked";
        let d = extract_click_directives(content);
        assert_eq!(d.len(), 1);
    }
}
