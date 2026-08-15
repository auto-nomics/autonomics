//! Diagnostic runner — orchestrates all rules and collects issues.
//!
//! Ported from dendrite; the only change is replacing the direct sqlx
//! query for knowledge-linking rows with Turso.

use std::collections::HashMap;

use turso::Value;
use uuid::Uuid;

use crate::Storage;
use crate::diagnostics::knowledge_rules::{NestedKnowledge, VagueTitle};
use crate::storage::repo;
use crate::storage::types::{Index, TargetType};

use super::entity_rules::EntityDiagnosticRule;
use super::index_rules::IndexDiagnosticRule;
use super::knowledge_rules::KnowledgeDiagnosticRule;
use super::{CodeDescription, Diagnostic, Severity};

// ── Location text rendering ──────────────────────────────────────

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum LocationLeafKind {
    Index,
    Knowledge,
    Entity,
}

fn leaf_kind_from_target(target_type: TargetType) -> LocationLeafKind {
    match target_type {
        TargetType::Group => LocationLeafKind::Index,
        TargetType::Knowledge => LocationLeafKind::Knowledge,
    }
}

fn format_location_with_leaf_marker(path: &str, leaf_kind: LocationLeafKind) -> String {
    let suffix = match leaf_kind {
        LocationLeafKind::Index => "",
        LocationLeafKind::Knowledge => " [knowledge]",
        LocationLeafKind::Entity => " [entity]",
    };
    if suffix.is_empty() {
        return path.to_string();
    }
    match path.rfind(" > ") {
        Some(idx) => {
            let (prefix, last) = path.split_at(idx + " > ".len());
            format!("{prefix}{last}{suffix}")
        }
        None => format!("{path}{suffix}"),
    }
}

// ── Index diagnostics ────────────────────────────────────────────

async fn run_index_diagnostics(
    storage: &Storage,
) -> Result<(Vec<Diagnostic>, HashMap<Uuid, String>), String> {
    use super::index_rules::{
        DuplicateSiblingTitles, EmptyLeaf, ExcessiveChildren, InconsistentPrefixes,
    };

    let rules: Vec<Box<dyn IndexDiagnosticRule>> = vec![
        Box::new(EmptyLeaf),
        Box::new(ExcessiveChildren),
        Box::new(DuplicateSiblingTitles),
        Box::new(InconsistentPrefixes),
    ];

    let mut issues = Vec::new();
    let mut path_map = HashMap::new();

    let conn = &*storage.conn().await;
    let root = repo::index_find_root(conn)
        .await
        .map_err(|e| e.to_string())?;
    let all_nodes = repo::index_list_all(conn)
        .await
        .map_err(|e| e.to_string())?;

    let mut children_map: HashMap<Option<Uuid>, Vec<Index>> = HashMap::new();
    for node in &all_nodes {
        children_map
            .entry(node.parent_id)
            .or_default()
            .push(node.clone());
    }

    let mut stack: Vec<(Index, usize, Vec<String>)> = vec![(root, 0, vec![])];

    while let Some((node, depth, path)) = stack.pop() {
        let title = node.title.as_deref().unwrap_or("(unnamed)");
        let mut current_path = path.clone();
        if depth > 0 {
            current_path.push(title.to_string());
        } else {
            current_path = vec!["Root".to_string()];
        }
        let raw_path = current_path.join(" > ");
        path_map.insert(node.id, raw_path.clone());
        let location =
            format_location_with_leaf_marker(&raw_path, leaf_kind_from_target(node.target_type));

        let children = children_map
            .get(&Some(node.id))
            .cloned()
            .unwrap_or_default();

        for rule in &rules {
            if let Some(d) = rule.check(&node, depth, &location, &children) {
                issues.push(d);
            }
        }

        for child in children.into_iter().rev() {
            stack.push((child, depth + 1, current_path.clone()));
        }
    }

    Ok((issues, path_map))
}

// ── Knowledge diagnostics ────────────────────────────────────────

async fn run_knowledge_diagnostics(
    storage: &Storage,
    index_path_map: &HashMap<Uuid, String>,
) -> Result<Vec<Diagnostic>, String> {
    use super::knowledge_rules::{
        BoldAsHeading, EmptyContent, NoEntities, OrphanKnowledge, TitleMissingEntityPrefix,
    };

    let rules: Vec<Box<dyn KnowledgeDiagnosticRule>> = vec![
        Box::new(BoldAsHeading),
        Box::new(OrphanKnowledge),
        Box::new(EmptyContent),
        Box::new(NoEntities),
        Box::new(TitleMissingEntityPrefix),
        Box::new(NestedKnowledge),
        Box::new(VagueTitle),
    ];

    let mut issues: Vec<Diagnostic> = Vec::new();
    let conn = &*storage.conn().await;

    let orphan_titles = repo::index_orphan_knowledge_titles(conn)
        .await
        .map_err(|e| e.to_string())?;

    // Fetch (index_id, target) pairs for knowledge-type entries via Turso.
    let mut linking_rows = conn
        .query(
            "SELECT id, target FROM indexes WHERE target_type = 'knowledge' AND target IS NOT NULL",
            turso::params_from_iter([] as [Value; 0]),
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut knowledge_location: HashMap<Uuid, String> = HashMap::new();
    loop {
        match linking_rows.next().await {
            Ok(Some(row)) => {
                let index_id_str = match row.get_value(0) {
                    Ok(Value::Text(s)) => s,
                    _ => continue,
                };
                let target_str = match row.get_value(1) {
                    Ok(Value::Text(s)) => s,
                    _ => continue,
                };
                if let (Ok(index_id), Ok(knowledge_id)) =
                    (Uuid::parse_str(&index_id_str), Uuid::parse_str(&target_str))
                {
                    if let Some(path) = index_path_map.get(&index_id) {
                        knowledge_location.insert(knowledge_id, path.clone());
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }

    let all_knowledges = repo::knowledge_list_all(conn)
        .await
        .map_err(|e| e.to_string())?;

    for k in all_knowledges {
        let raw_location = knowledge_location
            .get(&k.id)
            .cloned()
            .unwrap_or_else(|| format!("(orphan) {}", k.title));
        let location = format_location_with_leaf_marker(&raw_location, LocationLeafKind::Knowledge);

        for rule in &rules {
            if let Some(mut d) = rule.check(&k) {
                d.location = location.clone();
                issues.push(d);
            }
        }

        if orphan_titles.contains(&k.title) {
            let orphan_raw = knowledge_location
                .get(&k.id)
                .cloned()
                .unwrap_or_else(|| format!("(orphan) {}", k.title));
            let orphan_location =
                format_location_with_leaf_marker(&orphan_raw, LocationLeafKind::Knowledge);
            issues.push(Diagnostic {
                code: "knowledge.orphan".to_string(),
                code_description: Some(CodeDescription {
                    href: "kms://diagnostics/knowledge/orphan".to_string(),
                }),
                location: orphan_location,
                severity: Severity::Warning,
                message: "有知识条目但没有被任何索引节点引用".to_string(),
                suggested_actions: vec![
                    "使用 kms_link_orphans 将该知识条目链接到适当的索引节点下".to_string(),
                ],
            });
        }
    }

    Ok(issues)
}

// ── Entity diagnostics ───────────────────────────────────────────

async fn run_entity_diagnostics(storage: &Storage) -> Result<Vec<Diagnostic>, String> {
    use super::entity_rules::{EmptyDefinition, MissingZhNomenclature, NoNomenclature};

    let rules: Vec<Box<dyn EntityDiagnosticRule>> = vec![
        Box::new(NoNomenclature),
        Box::new(EmptyDefinition),
        Box::new(MissingZhNomenclature),
    ];

    let mut issues = Vec::new();
    let conn = &*storage.conn().await;

    let all_entities = repo::entity_list_all(conn)
        .await
        .map_err(|e| e.to_string())?;

    for entity in all_entities {
        for rule in &rules {
            if let Some(d) = rule.check(&entity) {
                issues.push(d);
            }
        }
    }

    Ok(issues)
}

// ── Entry point ──────────────────────────────────────────────────

pub async fn run_diagnostics(storage: &Storage) -> Result<Vec<Diagnostic>, String> {
    let mut all_issues = Vec::new();

    let (index_issues, index_path_map) = run_index_diagnostics(storage).await?;
    all_issues.extend(index_issues);

    let knowledge_issues = run_knowledge_diagnostics(storage, &index_path_map).await?;
    all_issues.extend(knowledge_issues);

    let entity_issues = run_entity_diagnostics(storage).await?;
    all_issues.extend(entity_issues);

    Ok(all_issues)
}
