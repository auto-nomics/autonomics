use std::collections::HashMap;

use crate::storage::types::{Index, TargetType};

use super::{CodeDescription, Diagnostic, Severity};

// ── Helpers ──────────────────────────────────────────────────────

fn extract_prefix(title: &str) -> &str {
    title.split(" · ").next().unwrap_or(title)
}

fn analyze_knowledge_prefixes(children: &[Index]) -> Option<Vec<(&str, usize)>> {
    // 只关注 Knowledge 类型的子节点
    let knowledge_children: Vec<&Index> = children
        .iter()
        .filter(|c| c.target_type == TargetType::Knowledge)
        .collect();

    if knowledge_children.len() < 3 {
        return None;
    }

    let mut prefix_counts: HashMap<&str, usize> = HashMap::new();
    for child in &knowledge_children {
        let title = child.title.as_deref().unwrap_or("");
        let prefix = extract_prefix(title);
        if prefix.is_empty() {
            continue;
        }
        *prefix_counts.entry(prefix).or_insert(0) += 1;
    }

    if prefix_counts.len() < 2 {
        return None;
    }

    let total_titled: usize = prefix_counts.values().sum();
    let max_count = *prefix_counts.values().max().unwrap();
    if max_count * 2 >= total_titled {
        return None;
    }

    let mut sorted: Vec<(&str, usize)> = prefix_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(5);

    Some(sorted)
}

// ── Trait ────────────────────────────────────────────────────────

pub trait IndexDiagnosticRule: Send + Sync {
    fn check(
        &self,
        node: &Index,
        depth: usize,
        location: &str,
        children: &[Index],
    ) -> Option<Diagnostic>;
    fn name(&self) -> &str;
}

// ── Rules ───────────────────────────────────────────────────────

pub struct EmptyLeaf;

impl IndexDiagnosticRule for EmptyLeaf {
    fn check(
        &self,
        node: &Index,
        depth: usize,
        location: &str,
        children: &[Index],
    ) -> Option<Diagnostic> {
        if depth > 0 && node.target_type == TargetType::Group && children.is_empty() {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/index/empty-leaf".to_string(),
                }),
                location: location.to_string(),
                severity: Severity::Warning,
                message: "没有子节点也没有关联知识".to_string(),
                suggested_actions: vec![
                    "为该节点创建知识索引，或导航到该节点后添加子节点".to_string(),
                    "如不再需要，可考虑删除该空节点".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "index.empty_leaf"
    }
}

pub struct ExcessiveChildren;

impl IndexDiagnosticRule for ExcessiveChildren {
    fn check(
        &self,
        _node: &Index,
        _depth: usize,
        location: &str,
        children: &[Index],
    ) -> Option<Diagnostic> {
        if children.len() > 6 {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/index/excessive-children".to_string(),
                }),
                location: location.to_string(),
                severity: Severity::Warning,
                message: format!("有 {} 个子节点，建议重构整理", children.len()),
                suggested_actions: vec![
                    "使用 kms_move_children 将子节点按主题分组".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "index.excessive_children"
    }
}

pub struct DuplicateSiblingTitles;

impl IndexDiagnosticRule for DuplicateSiblingTitles {
    fn check(
        &self,
        _node: &Index,
        _depth: usize,
        location: &str,
        children: &[Index],
    ) -> Option<Diagnostic> {
        use std::collections::HashMap;

        // Count occurrences of each titled child. Untitled children
        // (title == None) are not considered duplicates — there is no
        // name to compare.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for child in children {
            if let Some(title) = child.title.as_deref() {
                if title.is_empty() {
                    continue;
                }
                *seen.entry(title).or_insert(0) += 1;
            }
        }

        let mut duplicates: Vec<String> = seen
            .into_iter()
            .filter_map(|(title, count)| {
                if count > 1 {
                    Some(format!("「{}」({} 次)", title, count))
                } else {
                    None
                }
            })
            .collect();

        if duplicates.is_empty() {
            return None;
        }

        duplicates.sort();

        Some(Diagnostic {
            code: self.name().to_string(),
            code_description: Some(CodeDescription {
                href: "kms://diagnostics/index/duplicate-sibling-titles".to_string(),
            }),
            location: location.to_string(),
            severity: Severity::Error,
            message: format!(
                "同级索引节点存在重名: {}。同一父节点下子索引标题必须唯一，否则 `find_by_title` 等按名查找会歧义。",
                duplicates.join("、")
            ),
            suggested_actions: vec![
                "为重名的子节点补上唯一区分（例如加上切面/编号后缀）后重命名。".to_string(),
                "重命名后再调用 `kms_local` 确认无残留歧义。".to_string(),
            ],
        })
    }

    fn name(&self) -> &str {
        "index.duplicate_sibling_titles"
    }
}

pub struct InconsistentPrefixes;

impl IndexDiagnosticRule for InconsistentPrefixes {
    fn check(
        &self,
        node: &Index,
        _depth: usize,
        location: &str,
        children: &[Index],
    ) -> Option<Diagnostic> {
        let prefixes = analyze_knowledge_prefixes(children)?;

        let prefix_list: Vec<String> = prefixes
            .iter()
            .map(|(prefix, count)| format!("「{}」({} 个)", prefix, count))
            .collect();

        let parent_title = node.title.as_deref().unwrap_or("(unnamed)");

        Some(Diagnostic {
            code: self.name().to_string(),
            code_description: Some(CodeDescription {
                href: "kms://diagnostics/index/inconsistent-prefixes".to_string(),
            }),
            location: location.to_string(),
            severity: Severity::Hint,
            message: format!(
                "知识子节点标题前缀分布零散（{}），可能需要重组",
                prefix_list.join("、")
            ),
            suggested_actions: vec![
                "考虑将具有相同前缀的知识子节点归入新的分组节点下".to_string(),
                format!(
                    "例如：为「{}」下的子节点创建以各前缀命名的子分组",
                    parent_title,
                ),
            ],
        })
    }

    fn name(&self) -> &str {
        "index.inconsistent_prefixes"
    }
}
