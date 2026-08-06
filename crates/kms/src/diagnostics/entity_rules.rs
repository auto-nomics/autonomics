use crate::Language;
use crate::storage::types::Entity;

use super::{CodeDescription, Diagnostic, Severity};

// ── Trait ────────────────────────────────────────────────────────

pub trait EntityDiagnosticRule: Send + Sync {
    fn check(&self, entity: &Entity) -> Option<Diagnostic>;
    fn name(&self) -> &str;
}

fn format_entity_location(name: &str) -> String {
    // entity 诊断的 location 没有路径，只有一个实体名。
    // 追加 [entity] 标记，与 index/knowledge 诊断保持一致的区分风格。
    format!("{} [entity]", name)
}

// ── Rules ───────────────────────────────────────────────────────

pub struct NoNomenclature;

impl EntityDiagnosticRule for NoNomenclature {
    fn check(&self, entity: &Entity) -> Option<Diagnostic> {
        if entity.name.is_empty() {
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/entity/no-nomenclature".to_string(),
                }),
                location: format_entity_location("(unnamed entity)"),
                severity: Severity::Error,
                message: "实体没有任何命名".to_string(),
                suggested_actions: vec![
                    "使用 kms_delete_entity 删除该孤儿实体 (推荐)".to_string(),
                    "或使用 kms_update_entity 为其添加命名".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "entity.no_nomenclature"
    }
}

pub struct EmptyDefinition;

impl EntityDiagnosticRule for EmptyDefinition {
    fn check(&self, entity: &Entity) -> Option<Diagnostic> {
        if entity.definition.is_empty() {
            let name = entity
                .name
                .first()
                .map(|n| n.full.as_str())
                .unwrap_or("(unnamed)");
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/entity/empty-definition".to_string(),
                }),
                location: format_entity_location(name),
                severity: Severity::Warning,
                message: "实体定义为空".to_string(),
                suggested_actions: vec![
                    "使用 kms_update_entity 补充实体的定义".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "entity.empty_definition"
    }
}

pub struct MissingZhNomenclature;

impl EntityDiagnosticRule for MissingZhNomenclature {
    fn check(&self, entity: &Entity) -> Option<Diagnostic> {
        let has_zh = entity.name.iter().any(|n| matches!(n.lang, Language::ZH));
        if !has_zh {
            let name = entity
                .name
                .first()
                .map(|n| n.full.as_str())
                .unwrap_or("(unnamed)");
            Some(Diagnostic {
                code: self.name().to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/entity/missing-zh-nomenclature".to_string(),
                }),
                location: format_entity_location(name),
                severity: Severity::Hint,
                message: "实体缺少中文命名".to_string(),
                suggested_actions: vec![
                    "为实体添加中文 (ZH) 命名".to_string(),
                ],
            })
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "entity.missing_zh_nomenclature"
    }
}
