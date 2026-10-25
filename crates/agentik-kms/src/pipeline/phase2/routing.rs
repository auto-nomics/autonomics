use std::collections::{BTreeMap, HashMap, HashSet};

use crate::KnowledgeRoutingDecision;
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::store::{MemoryNote, MemoryStage1Record};

use super::KnowledgeUpdateInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutingAction {
    RewriteExisting,
    CreateChild,
    CreateSibling,
    CreateNew,
    Discard,
}

impl RoutingAction {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "rewrite_existing" => Some(Self::RewriteExisting),
            "create_child" => Some(Self::CreateChild),
            "create_sibling" => Some(Self::CreateSibling),
            "create_new" => Some(Self::CreateNew),
            "discard" => Some(Self::Discard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RoutedUpdate {
    pub update: KnowledgeUpdateInput,
    pub action: RoutingAction,
    pub anchor_path: Option<KnowledgePath>,
    pub target_path: Option<KnowledgePath>,
    pub reason: String,
    pub confidence: f64,
}

pub(super) fn updates_from_inputs(
    rows: &[MemoryStage1Record],
    notes: &[MemoryNote],
) -> Vec<KnowledgeUpdateInput> {
    let mut updates = Vec::new();
    for row in rows {
        let mut content = format!("### Raw memory\n{}\n", row.raw_memory);
        if !row.rollout_summary.is_empty() {
            content.push_str(&format!("### Rollout summary\n{}\n", row.rollout_summary));
        }
        updates.push(KnowledgeUpdateInput {
            id: format!("session:{}", row.session_id),
            kind: "session_extract".to_string(),
            content,
        });
    }
    for note in notes {
        updates.push(KnowledgeUpdateInput {
            id: format!("note:{}", note.id),
            kind: "memory_note".to_string(),
            content: note.content.clone(),
        });
    }
    updates
}

pub(super) fn validate_plan(
    decisions: Vec<KnowledgeRoutingDecision>,
    updates: &[KnowledgeUpdateInput],
    existing_nodes: &[Knowledge],
) -> Result<Vec<RoutedUpdate>, String> {
    let update_ids = updates
        .iter()
        .map(|update| update.id.as_str())
        .collect::<HashSet<_>>();
    let existing_paths = existing_nodes
        .iter()
        .map(|node| node.knowledge_path.clone())
        .collect::<HashSet<_>>();
    let mut by_update_id = decisions
        .into_iter()
        .map(|decision| (decision.update_id.clone(), decision))
        .collect::<HashMap<String, KnowledgeRoutingDecision>>();

    if by_update_id.len() != update_ids.len() {
        return Err("routing plan must contain exactly one decision per update".to_string());
    }

    let mut routed = Vec::new();
    for update in updates {
        let Some(decision) = by_update_id.remove(update.id.as_str()) else {
            return Err(format!("routing plan is missing update {}", update.id));
        };
        if !update_ids.contains(decision.update_id.as_str()) {
            return Err(format!("unknown routing update {}", decision.update_id));
        }
        let action = RoutingAction::parse(&decision.action)
            .ok_or_else(|| format!("unknown routing action {}", decision.action))?;
        let anchor = match decision.anchor_path.as_deref() {
            Some(raw) => {
                let path = KnowledgePath::normalize(raw)
                    .ok_or_else(|| format!("invalid anchor path {raw}"))?;
                if !existing_paths.contains(&path) {
                    return Err(format!("anchor path {raw} does not exist"));
                }
                Some(path)
            }
            None => None,
        };
        let target = match decision.target_path.as_deref() {
            Some(raw) => Some(
                KnowledgePath::normalize(raw)
                    .ok_or_else(|| format!("invalid target path {raw}"))?,
            ),
            None => None,
        };

        match action {
            RoutingAction::Discard => {}
            RoutingAction::RewriteExisting => {
                let (Some(anchor), Some(target)) = (&anchor, &target) else {
                    return Err(format!(
                        "routing action {decision_action} requires valid anchor and target paths",
                        decision_action = decision.action
                    ));
                };
                if anchor != target || !existing_paths.contains(target) {
                    return Err(format!(
                        "rewrite_existing target {} must equal an existing node",
                        target.as_str()
                    ));
                }
            }
            RoutingAction::CreateChild => {
                let (Some(anchor), Some(target)) = (&anchor, &target) else {
                    return Err(format!(
                        "routing action {decision_action} requires valid anchor and target paths",
                        decision_action = decision.action
                    ));
                };
                if existing_paths.contains(target) || !is_descendant(target, anchor) {
                    return Err(format!(
                        "create_child target {} must be a new descendant of {}",
                        target.as_str(),
                        anchor.as_str()
                    ));
                }
            }
            RoutingAction::CreateSibling => {
                let (Some(anchor), Some(target)) = (&anchor, &target) else {
                    return Err(format!(
                        "routing action {decision_action} requires valid anchor and target paths",
                        decision_action = decision.action
                    ));
                };
                if existing_paths.contains(target)
                    || anchor.parent().as_ref() != target.parent().as_ref()
                {
                    return Err(format!(
                        "create_sibling target {} must be a new sibling of {}",
                        target.as_str(),
                        anchor.as_str()
                    ));
                }
            }
            RoutingAction::CreateNew => {
                let Some(target) = target.as_ref() else {
                    return Err(format!(
                        "routing action {decision_action} requires a valid target path",
                        decision_action = decision.action
                    ));
                };
                if existing_paths.contains(target) {
                    return Err(format!(
                        "create_new target {} must not already exist",
                        target.as_str()
                    ));
                }
            }
        }

        routed.push(RoutedUpdate {
            update: update.clone(),
            action,
            anchor_path: anchor,
            target_path: target,
            reason: decision.reason,
            confidence: decision.confidence.clamp(0.0, 1.0),
        });
    }

    routed.sort_by(|a, b| {
        a.target_path
            .as_ref()
            .map(|path| path.as_str())
            .unwrap_or_default()
            .cmp(
                &b.target_path
                    .as_ref()
                    .map(|path| path.as_str())
                    .unwrap_or_default(),
            )
            .then_with(|| a.update.id.cmp(&b.update.id))
    });
    Ok(routed)
}

pub(crate) fn group_by_target(routed: Vec<RoutedUpdate>) -> BTreeMap<String, Vec<RoutedUpdate>> {
    let mut groups: BTreeMap<String, Vec<RoutedUpdate>> = BTreeMap::new();
    for update in routed {
        if let Some(target) = update.target_path.clone() {
            groups.entry(target.as_str()).or_default().push(update);
        }
    }
    groups
}

fn is_descendant(path: &KnowledgePath, ancestor: &KnowledgePath) -> bool {
    path.segments().len() > ancestor.segments().len()
        && path.segments()[..ancestor.segments().len()] == *ancestor.segments()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> KnowledgePath {
        KnowledgePath::normalize(raw).unwrap()
    }

    fn update(id: &str) -> KnowledgeUpdateInput {
        KnowledgeUpdateInput {
            id: id.to_string(),
            kind: "test".to_string(),
            content: "content".to_string(),
        }
    }

    fn decision(
        id: &str,
        action: &str,
        anchor: Option<&str>,
        target: Option<&str>,
    ) -> KnowledgeRoutingDecision {
        KnowledgeRoutingDecision {
            update_id: id.to_string(),
            action: action.to_string(),
            anchor_path: anchor.map(ToString::to_string),
            target_path: target.map(ToString::to_string),
            reason: String::new(),
            confidence: 0.8,
        }
    }

    #[test]
    fn validates_existing_child_sibling_and_new_routes() {
        let updates = vec![update("a"), update("b"), update("c"), update("d")];
        let nodes = vec![
            Knowledge::leaf(
                path("/engineering/testing"),
                "Testing".into(),
                "Old".into(),
                0.8,
                "s",
                1,
            ),
            Knowledge::leaf(
                path("/user/preferences"),
                "Prefs".into(),
                "Old".into(),
                0.8,
                "s",
                1,
            ),
        ];
        let decisions = vec![
            decision(
                "a",
                "rewrite_existing",
                Some("/engineering/testing"),
                Some("/engineering/testing"),
            ),
            decision(
                "b",
                "create_child",
                Some("/engineering/testing"),
                Some("/engineering/testing/verification"),
            ),
            decision(
                "c",
                "create_sibling",
                Some("/engineering/testing"),
                Some("/engineering/deployment"),
            ),
            decision("d", "create_new", None, Some("/statistics/regression")),
        ];

        let routed = validate_plan(decisions, &updates, &nodes).unwrap();
        assert_eq!(routed.len(), 4);
    }

    #[test]
    fn rejects_rewrite_to_missing_node() {
        let updates = vec![update("a")];
        let decisions = vec![decision(
            "a",
            "rewrite_existing",
            Some("/engineering/testing"),
            Some("/engineering/testing"),
        )];
        assert!(validate_plan(decisions, &updates, &[]).is_err());
    }
}
