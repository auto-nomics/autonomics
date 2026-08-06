use std::sync::LazyLock;

use crate::storage::types::Knowledge;

use super::{CodeDescription, Diagnostic, Severity};
use regex::Regex;

// ── Trait ────────────────────────────────────────────────────────

pub trait KnowledgeDiagnosticRule: Send + Sync {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic>;
    fn name(&self) -> &str;
}

// ── Rules ───────────────────────────────────────────────────────

pub struct NestedKnowledge;

static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#{1,6} .+").unwrap());

const VAGUE_TITLE_KEYWORDS: &[&str] = &[
    "概述",
    "总结",
    "小结",
    "定义",
    "简介",
    "说明",
    "介绍",
    "基本概念",
    "特征",
];

impl KnowledgeDiagnosticRule for NestedKnowledge {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        // let mut options = Options::empty();
        // let parser = Parser::new_ext(
        //     knowledge.content.clone().unwrap_or("".to_string()).as_str(),
        //     options,
        // );

        let content = knowledge.content.as_deref().unwrap_or("");
        let headings: Vec<&str> = HEADING_RE.find_iter(content).map(|m| m.as_str()).collect();

        if headings.len() > 1 {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/knowledge/internal-nested".to_string(),
                }),
                location: knowledge.title.clone(),
                severity: Severity::Warning,
                message: "知识条目内容包含多级标题嵌套".to_string(),
                suggested_actions: vec![
                    "知识条目内容应保持扁平结构，仅使用单一标题层级".to_string(),
                    "将嵌套的知识层级转移到 index 索引树中，使用 kms_create_index 创建子节点"
                        .to_string(),
                    "每个 index 节点应对应一个独立、针对性的知识条目".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "knowledge.internal_nested"
    }
}

pub struct VagueTitle;

impl KnowledgeDiagnosticRule for VagueTitle {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        // 提取标题中实体名之后的部分（" · " 分隔符之后）
        let suffix = knowledge
            .title
            .split(" · ")
            .nth(1)
            .unwrap_or(&knowledge.title);

        let matched = VAGUE_TITLE_KEYWORDS
            .iter()
            .find(|&&keyword| suffix.contains(keyword))?;

        Some(Diagnostic {
            code: self.name().to_string(),
            code_description: Some(CodeDescription {
                href: "kms://diagnostics/knowledge/vague-title".to_string(),
            }),
            location: knowledge.title.clone(),
            severity: Severity::Warning,
            message: format!("知识标题包含模糊描述 \"{}\"，缺乏针对性", matched),
            suggested_actions: vec![
                format!(
                    "将 \"{}\" 替换为更具体的方面描述，如 \"核心原理\"、\"使用场景\"",
                    matched
                ),
                "知识条目标题应清晰表达该条目涵盖的具体内容".to_string(),
            ],
        })
    }

    fn name(&self) -> &str {
        "knowledge.vague_title"
    }
}

pub struct OrphanKnowledge;

impl KnowledgeDiagnosticRule for OrphanKnowledge {
    fn check(&self, _knowledge: &Knowledge) -> Option<Diagnostic> {
        None
    }

    fn name(&self) -> &str {
        "knowledge.orphan"
    }
}

pub struct EmptyContent;

impl KnowledgeDiagnosticRule for EmptyContent {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        match &knowledge.content {
            Some(c) if c.is_empty() => Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/knowledge/empty-content".to_string(),
                }),
                location: knowledge.title.clone(),
                severity: Severity::Hint,
                message: "知识条目内容为空".to_string(),
                suggested_actions: vec![
                    "使用 kms_update_knowledge 补充该知识条目的内容".to_string(),
                ],
            }),
            _ => None,
        }
    }

    fn name(&self) -> &str {
        "knowledge.empty_content"
    }
}

pub struct NoEntities;

impl KnowledgeDiagnosticRule for NoEntities {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        if knowledge.entities.is_empty() {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/knowledge/no-entities".to_string(),
                }),
                location: knowledge.title.clone(),
                severity: Severity::Warning,
                message: "知识条目没有关联任何实体".to_string(),
                suggested_actions: vec![
                    "该知识条目应关联至少一个实体，请在创建时指定 entities 字段".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "knowledge.no_entities"
    }
}

static BOLD_ONLY_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\*\*.+\*\*\s*$").unwrap());

pub struct BoldAsHeading;

impl KnowledgeDiagnosticRule for BoldAsHeading {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        let content = knowledge.content.as_deref().unwrap_or("");
        let matches: Vec<&str> = BOLD_ONLY_LINE_RE.find_iter(content).map(|m| m.as_str()).collect();
        let offending = &matches[0..matches.len().min(3)];
        if matches.is_empty() {
            return None;
        }
        Some(Diagnostic {
            code: self.name().to_string(),
            code_description: Some(CodeDescription {
                href: "kms://diagnostics/knowledge/bold-as-heading".to_string(),
            }),
            location: knowledge.title.clone(),
            severity: Severity::Error,
            message: format!(
                "使用粗体文本作为标题（共{}处）：{}",
                matches.len(),
                offending.join(", ")
            ),
            suggested_actions: vec![
                "将粗体文本替换为 Markdown 标题语法：`**标题**` → `## 标题`".to_string(),
                "粗体 `**文本**` 仅用于行内强调，不可独占一行充当标题".to_string(),
            ],
        })
    }

    fn name(&self) -> &str {
        "knowledge.bold_as_heading"
    }
}

pub struct TitleMissingEntityPrefix;

impl KnowledgeDiagnosticRule for TitleMissingEntityPrefix {
    fn check(&self, knowledge: &Knowledge) -> Option<Diagnostic> {
        if !knowledge.title.contains(" · ") {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/knowledge/title-missing-entity-prefix".to_string(),
                }),
                location: knowledge.title.clone(),
                severity: Severity::Warning,
                message: "知识标题未遵循 \"实体名 · 方面描述\" 命名规范".to_string(),
                suggested_actions: vec![
                    "知识标题应包含实体名前缀，格式如 \"React · 核心原理\"".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "knowledge.title_missing_entity_prefix"
    }
}
