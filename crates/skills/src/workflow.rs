//! Workflow templates: parameterized DAG definitions inside a skill.
//!
//! A skill may carry `workflow/<name>.toml` files describing a DAG as
//! node kinds + specs + port-to-port edges, with `{{param}}`
//! placeholders inside spec strings. Rendering fills placeholders from
//! caller-supplied parameters (defaults applied, types checked), and
//! the resulting nodes/edges are added to the session's DAG engine by
//! the runtime tooling — the same code path the agent's own
//! `add_node` / `add_edge` calls use.
//!
//! This module is deliberately engine-agnostic: it parses, validates,
//! and renders. Binding to [`data_engine::runtime::DataEngineClient`]
//! lives in the runtime crate.
//!
//! ```toml
//! description = "Linear regression over a CSV table."
//!
//! [params]
//! input = { type = "string", description = "CSV path", required = true }
//! intercept = { type = "boolean", default = true }
//!
//! [[node]]
//! id = "read"
//! kind = "file_to_dataframe"
//! spec = { path = "{{input}}", format = "csv" }
//!
//! [[node]]
//! id = "fit"
//! kind = "linear_regression"
//! spec = { x_columns = ["x1"], y_column = "y", intercept = "{{intercept}}" }
//!
//! [[edge]]
//! from = "read"
//! from_port = 0
//! to = "fit"
//! to_port = 0
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SkillError;

/// Directory inside a skill holding workflow templates.
pub const WORKFLOW_DIR: &str = "workflow";

/// One declared template parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    /// `string` | `integer` | `number` | `boolean`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// One node declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeDecl {
    pub id: String,
    pub kind: String,
    /// JSON-shaped spec; string leaves may carry `{{param}}`.
    pub spec: Value,
}

/// One port-to-port edge declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDecl {
    pub from: String,
    pub from_port: u8,
    pub to: String,
    pub to_port: u8,
}

/// A parsed workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    #[serde(default, rename = "node")]
    pub nodes: Vec<NodeDecl>,
    #[serde(default, rename = "edge")]
    pub edges: Vec<EdgeDecl>,
}

/// A fully rendered workflow: concrete node specs, ready for the DAG
/// engine.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedWorkflow {
    pub nodes: Vec<NodeDecl>,
    pub edges: Vec<EdgeDecl>,
}

impl WorkflowTemplate {
    /// Parse and structurally validate a template from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, SkillError> {
        let template: WorkflowTemplate = toml::from_str(text).map_err(|e| {
            SkillError::invalid_frontmatter("<workflow>", format!("invalid workflow TOML: {e}"))
        })?;
        template.validate()?;
        Ok(template)
    }

    /// Parse the template file `dir/workflow/<stem>.toml`.
    pub fn load(dir: &Path, stem: &str) -> Result<Self, SkillError> {
        let path = dir.join(WORKFLOW_DIR).join(format!("{stem}.toml"));
        let text = std::fs::read_to_string(&path).map_err(|e| SkillError::Unreadable {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        Self::from_toml(&text).map_err(|e| match e {
            SkillError::InvalidFrontmatter { reason, .. } => {
                SkillError::invalid_frontmatter(path, reason)
            }
            other => other,
        })
    }

    fn validate(&self) -> Result<(), SkillError> {
        let err = |reason: String| SkillError::invalid_frontmatter("<workflow>", reason);
        if self.nodes.is_empty() {
            return Err(err("workflow declares no [[node]] entries".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(err("node with empty id".into()));
            }
            if !seen.insert(node.id.clone()) {
                return Err(err(format!("duplicate node id {:?}", node.id)));
            }
            if !node.spec.is_object() {
                return Err(err(format!(
                    "node {:?}: spec must be a table/JSON object",
                    node.id
                )));
            }
        }
        for edge in &self.edges {
            if !seen.contains(&edge.from) {
                return Err(err(format!(
                    "edge references unknown 'from' node {:?}",
                    edge.from
                )));
            }
            if !seen.contains(&edge.to) {
                return Err(err(format!(
                    "edge references unknown 'to' node {:?}",
                    edge.to
                )));
            }
        }
        for (name, param) in &self.params {
            if !matches!(
                param.kind.as_str(),
                "string" | "integer" | "number" | "boolean"
            ) {
                return Err(err(format!(
                    "param {name:?}: unsupported type {:?} \
                     (expected string|integer|number|boolean)",
                    param.kind
                )));
            }
            if param.required && param.default.is_some() {
                return Err(err(format!(
                    "param {name:?}: required params cannot carry a default"
                )));
            }
        }
        Ok(())
    }

    /// Workflow template file stems available under `dir/workflow/`,
    /// sorted. Missing directory → empty.
    pub fn stems_in_dir(dir: &Path) -> Vec<String> {
        list_toml_stems(&dir.join(WORKFLOW_DIR))
    }

    /// Render with caller-supplied parameters.
    ///
    /// Applies defaults, enforces required params, type-checks values,
    /// and substitutes `{{param}}` occurrences in spec string leaves.
    /// Placeholders referencing undeclared params are errors — a typo
    /// in a template must fail loudly, not silently render literal
    /// braces into a node spec.
    pub fn render(&self, params: &Value) -> Result<RenderedWorkflow, SkillError> {
        let err = |reason: String| SkillError::invalid_frontmatter("<workflow>", reason);
        let empty = serde_json::Map::new();
        let supplied: &serde_json::Map<String, Value> = match params {
            Value::Null => &empty,
            Value::Object(map) => map,
            _ => return Err(err("params must be a JSON object".into())),
        };

        let mut resolved: serde_json::Map<String, Value> = Default::default();
        for (name, spec) in &self.params {
            let value = match supplied.get(name) {
                Some(v) => Some(v.clone()),
                None => spec.default.clone(),
            };
            let Some(value) = value else {
                if spec.required {
                    return Err(err(format!(
                        "missing required param {name:?} \
                         ({})",
                        spec.description
                    )));
                }
                continue;
            };
            check_param_type(name, &spec.kind, &value).map_err(&err)?;
            resolved.insert(name.clone(), value);
        }
        for key in supplied.keys() {
            if !self.params.contains_key(key) {
                return Err(err(format!(
                    "unknown param {key:?}; declared: {}",
                    self.params.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
        }

        let declared: Vec<&str> = self.params.keys().map(String::as_str).collect();
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let spec = substitute(&node.spec, &resolved, &declared).map_err(err)?;
            nodes.push(NodeDecl {
                id: node.id.clone(),
                kind: node.kind.clone(),
                spec,
            });
        }
        Ok(RenderedWorkflow {
            nodes,
            edges: self.edges.clone(),
        })
    }
}

fn check_param_type(name: &str, kind: &str, value: &Value) -> Result<(), String> {
    let ok = match (kind, value) {
        ("string", Value::String(_)) => true,
        ("integer", Value::Number(n)) => n.is_u64() || n.is_i64(),
        ("number", Value::Number(_)) => true,
        ("boolean", Value::Bool(_)) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "param {name:?} must be a {kind}, got {}",
            json_type_name(value)
        ))
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Substitute `{{param}}` in every string leaf of a spec value.
fn substitute(
    value: &Value,
    resolved: &serde_json::Map<String, Value>,
    declared: &[&str],
) -> Result<Value, String> {
    match value {
        Value::String(text) => {
            let mut out = String::with_capacity(text.len());
            let mut rest = text.as_str();
            while let Some(start) = rest.find("{{") {
                let Some(end_rel) = rest[start..].find("}}") else {
                    break;
                };
                let name = rest[start + 2..start + end_rel].trim();
                if !declared.contains(&name) {
                    return Err(format!(
                        "unknown placeholder {{{{{name}}}}} \
                         (declared params: {})",
                        declared.join(", ")
                    ));
                }
                let replacement = match resolved.get(name) {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => {
                        return Err(format!(
                            "placeholder {{{{{name}}}}} refers to an unset \
                             optional param"
                        ));
                    }
                };
                out.push_str(&rest[..start]);
                out.push_str(&replacement);
                rest = &rest[start + end_rel + 2..];
            }
            out.push_str(rest);
            Ok(Value::String(out))
        }
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| substitute(item, resolved, declared))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                out.insert(key.clone(), substitute(item, resolved, declared)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// List `*.toml` file stems in `dir`, sorted. Missing dir → empty.
pub(crate) fn list_toml_stems(dir: &Path) -> Vec<String> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stems: Vec<String> = read_dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let stem = path.file_stem()?.to_str()?;
            if name.starts_with('.') || !name.ends_with(".toml") || stem.is_empty() {
                return None;
            }
            Some(stem.to_string())
        })
        .collect();
    stems.sort();
    stems
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEMPLATE: &str = r#"
description = "Regression over a CSV table."

[params]
input = { type = "string", description = "CSV path", required = true }
n_blocks = { type = "integer", default = 200 }

[[node]]
id = "read"
kind = "file_to_dataframe"
spec = { path = "{{input}}", format = "csv" }

[[node]]
id = "fit"
kind = "linear_regression"
spec = { x_columns = ["x1"], y_column = "y", n_blocks = "{{n_blocks}}" }

[[edge]]
from = "read"
from_port = 0
to = "fit"
to_port = 0
"#;

    #[test]
    fn parses_and_renders_with_defaults() {
        let template = WorkflowTemplate::from_toml(TEMPLATE).unwrap();
        let rendered = template
            .render(&json!({ "input": "/data/table.csv" }))
            .unwrap();
        assert_eq!(rendered.nodes.len(), 2);
        assert_eq!(rendered.edges.len(), 1);
        assert_eq!(rendered.nodes[0].spec["path"], json!("/data/table.csv"));
        // Default applied through the placeholder.
        assert_eq!(rendered.nodes[1].spec["n_blocks"], json!("200"));
    }

    #[test]
    fn render_enforces_required_and_types() {
        let template = WorkflowTemplate::from_toml(TEMPLATE).unwrap();
        assert!(template.render(&json!({})).is_err());
        assert!(
            template
                .render(&json!({ "input": "/x.csv", "n_blocks": "lots" }))
                .is_err()
        );
        assert!(
            template
                .render(&json!({ "input": "/x.csv", "extra": 1 }))
                .is_err()
        );
    }

    #[test]
    fn validate_rejects_structural_errors() {
        assert!(WorkflowTemplate::from_toml("[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = {}\n[[edge]]\nfrom = \"a\"\nfrom_port = 0\nto = \"ghost\"\nto_port = 0\n").is_err());
        assert!(WorkflowTemplate::from_toml("[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = {}\n[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = {}\n").is_err());
        assert!(WorkflowTemplate::from_toml("[params]\nx = { type = \"float\" }\n[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = {}\n").is_err());
        assert!(WorkflowTemplate::from_toml("").is_err());
    }

    #[test]
    fn unknown_placeholder_is_an_error() {
        let text = "[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = { path = \"{{nope}}\" }\n";
        let template = WorkflowTemplate::from_toml(text).unwrap();
        let err = template.render(&json!({})).unwrap_err().to_string();
        assert!(err.contains("unknown placeholder"), "{err}");
    }

    #[test]
    fn optional_unset_placeholder_is_an_error_not_silent_garbage() {
        let text = "[params]\nopt = { type = \"string\" }\n[[node]]\nid = \"a\"\nkind = \"sql\"\nspec = { path = \"{{opt}}\" }\n";
        let template = WorkflowTemplate::from_toml(text).unwrap();
        // `opt` is declared but optional and unset → error, not a
        // literal "{{opt}}" leaking into the spec.
        assert!(template.render(&json!({})).is_err());
        let ok = template.render(&json!({ "opt": "x" })).unwrap();
        assert_eq!(ok.nodes[0].spec["path"], json!("x"));
    }

    #[test]
    fn loads_from_skill_dir_and_lists_stems() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = tmp.path().join(WORKFLOW_DIR);
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("main.toml"), TEMPLATE).unwrap();
        std::fs::write(wf.join(".hidden.toml"), "junk").unwrap();

        let stems = WorkflowTemplate::stems_in_dir(tmp.path());
        assert_eq!(stems, vec!["main".to_string()]);
        assert!(WorkflowTemplate::load(tmp.path(), "main").is_ok());
        assert!(WorkflowTemplate::load(tmp.path(), "missing").is_err());
    }
}
