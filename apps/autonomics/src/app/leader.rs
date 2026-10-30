//! LazyVim-style leader keys: `<Space>` enters a pending-prefix state, the
//! next key (or key sequence, for groups) resolves to an action, and a
//! which-key hint strip shows what the current prefix offers.
//!
//! Actions reuse the command palette's [`CommandAction`] verbatim, so a
//! leader binding and its palette entry can never drift apart.

use super::*;
use crate::widgets::command_palette::CommandAction;

/// One node of the leader key tree.
pub enum LeaderNode {
    /// A prefix that waits for one more key.
    Group {
        key: char,
        label: &'static str,
        children: &'static [LeaderNode],
    },
    /// A terminal binding that dispatches an action.
    Leaf {
        key: char,
        label: &'static str,
        action: CommandAction,
    },
}

impl LeaderNode {
    pub fn key(&self) -> char {
        match self {
            Self::Group { key, .. } | Self::Leaf { key, .. } => *key,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Group { label, .. } | Self::Leaf { label, .. } => label,
        }
    }
}

/// The root menu. `<Space><Space>` opens the palette, mirroring LazyVim's
/// `<leader><leader>` smart-find slot; groups cluster the rest.
pub(super) const LEADER_ROOT: &[LeaderNode] = &[
    LeaderNode::Leaf {
        key: ' ',
        label: "命令面板",
        action: CommandAction::OpenCommandPalette,
    },
    LeaderNode::Leaf {
        key: 'q',
        label: "退出",
        action: CommandAction::Quit,
    },
    LeaderNode::Group {
        key: 'a',
        label: "agent",
        children: &[
            LeaderNode::Leaf {
                key: 'n',
                label: "新建",
                action: CommandAction::NewAgent,
            },
            LeaderNode::Leaf {
                key: 'r',
                label: "恢复",
                action: CommandAction::ResumeAgent,
            },
            LeaderNode::Leaf {
                key: 't',
                label: "切换器",
                action: CommandAction::FocusAgentsPanel,
            },
            LeaderNode::Leaf {
                key: 'c',
                label: "关闭",
                action: CommandAction::CloseAgent,
            },
            LeaderNode::Leaf {
                key: 'd',
                label: "删除",
                action: CommandAction::DeleteAgent,
            },
            LeaderNode::Leaf {
                key: 'x',
                label: "取消",
                action: CommandAction::CancelAgent,
            },
        ],
    },
    LeaderNode::Group {
        key: 'v',
        label: "视图",
        children: &[
            LeaderNode::Leaf {
                key: 'd',
                label: "DAG",
                action: CommandAction::ViewDag,
            },
            LeaderNode::Leaf {
                key: 'b',
                label: "文献库",
                action: CommandAction::BibliographyPanel,
            },
            LeaderNode::Leaf {
                key: 's',
                label: "进化面板",
                action: CommandAction::SkillEvolutionPanel,
            },
            LeaderNode::Leaf {
                key: 'k',
                label: "知识库",
                action: CommandAction::KnowledgeWikiPanel,
            },
            LeaderNode::Leaf {
                key: 't',
                label: "纯对话视图",
                action: CommandAction::ToggleChatOnly,
            },
            LeaderNode::Leaf {
                key: 'h',
                label: "折叠思考",
                action: CommandAction::ToggleCollapseThinking,
            },
            LeaderNode::Leaf {
                key: 'c',
                label: "折叠工具调用",
                action: CommandAction::ToggleCollapseToolCalls,
            },
            LeaderNode::Leaf {
                key: 'r',
                label: "折叠工具结果",
                action: CommandAction::ToggleCollapseToolResults,
            },
        ],
    },
    LeaderNode::Leaf {
        key: 'm',
        label: "模型配置",
        action: CommandAction::ModelConfig,
    },
    LeaderNode::Leaf {
        key: 'e',
        label: "嵌入配置",
        action: CommandAction::EmbeddingConfig,
    },
    LeaderNode::Leaf {
        key: 'g',
        label: "agent 配置",
        action: CommandAction::AgentConfig,
    },
    LeaderNode::Leaf {
        key: 'h',
        label: "历史搜索",
        action: CommandAction::HistorySearch,
    },
    LeaderNode::Leaf {
        key: 'y',
        label: "复制消息",
        action: CommandAction::CopyMessage,
    },
    LeaderNode::Leaf {
        key: 'C',
        label: "压缩上下文",
        action: CommandAction::Compact,
    },
    LeaderNode::Leaf {
        key: 'G',
        label: "跟随输出",
        action: CommandAction::ToggleAutoScroll,
    },
    LeaderNode::Leaf {
        key: 'p',
        label: "计划弹窗",
        action: CommandAction::OpenPlanPopup,
    },
    LeaderNode::Leaf {
        key: 'T',
        label: "任务弹窗",
        action: CommandAction::OpenTasksPopup,
    },
];

/// Outcome of feeding one key to a pending leader prefix.
pub enum LeaderResolve {
    /// The key extends the prefix; the hint strip shows this group's
    /// children. The slice itself is re-derived from the prefix, so the
    /// payload is unused.
    Deeper(#[allow(dead_code)] &'static [LeaderNode]),
    /// The key completes a binding.
    Hit(CommandAction),
    /// No binding at this prefix; the pending prefix is dropped.
    Miss,
}

/// Resolve `key` against the children reachable after `prefix`
/// (empty prefix = the root menu).
pub(super) fn resolve(prefix: &[char], key: char) -> LeaderResolve {
    let children = children_of(prefix).unwrap_or(LEADER_ROOT);
    match children.iter().find(|node| node.key() == key) {
        Some(LeaderNode::Group { children, .. }) => LeaderResolve::Deeper(children),
        Some(LeaderNode::Leaf { action, .. }) => LeaderResolve::Hit(action.clone()),
        None => LeaderResolve::Miss,
    }
}

/// The children of the group at `prefix`, or the root menu for an empty
/// prefix. `None` when `prefix` does not name a group (a stale prefix).
pub(super) fn children_of(prefix: &[char]) -> Option<&'static [LeaderNode]> {
    let mut children = LEADER_ROOT;
    for &key in prefix {
        match children.iter().find(|node| node.key() == key) {
            Some(LeaderNode::Group {
                children: deeper, ..
            }) => children = deeper,
            _ => return None,
        }
    }
    Some(children)
}

/// One display row of the which-key panel.
pub(super) struct WhichKeyRow {
    pub key: char,
    pub label: &'static str,
    pub is_group: bool,
    /// Group only: number of direct children.
    pub children: usize,
}

/// Rows for the panel at `prefix`: leaves first, then groups — the same
/// order LazyVim's which-key panel uses.
pub(super) fn whichkey_rows(prefix: &[char]) -> Vec<WhichKeyRow> {
    let Some(children) = children_of(prefix) else {
        return Vec::new();
    };
    let mut rows: Vec<WhichKeyRow> = Vec::new();
    for node in children {
        match node {
            LeaderNode::Leaf { key, label, .. } => rows.push(WhichKeyRow {
                key: *key,
                label,
                is_group: false,
                children: 0,
            }),
            LeaderNode::Group {
                key,
                label,
                children,
            } => rows.push(WhichKeyRow {
                key: *key,
                label,
                is_group: true,
                children: children.len(),
            }),
        }
    }
    // Stable sort: leaves (is_group = false) before groups (true),
    // insertion order preserved within each class.
    rows.sort_by_key(|row| row.is_group);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(prefix: &[char], key: char) -> Option<CommandAction> {
        match resolve(prefix, key) {
            LeaderResolve::Hit(action) => Some(action),
            _ => None,
        }
    }

    #[test]
    fn double_space_opens_the_palette() {
        assert_eq!(hit(&[], ' '), Some(CommandAction::OpenCommandPalette));
    }

    #[test]
    fn groups_resolve_two_levels_deep() {
        // <leader>a n → new agent; <leader>v t → chat-only toggle.
        assert_eq!(hit(&['a'], 'n'), Some(CommandAction::NewAgent));
        assert_eq!(hit(&['v'], 't'), Some(CommandAction::ToggleChatOnly));
        // A leaf at depth 1 as well.
        assert_eq!(hit(&[], 'q'), Some(CommandAction::Quit));
    }

    #[test]
    fn unknown_keys_miss_without_panicking() {
        assert!(matches!(resolve(&[], 'z'), LeaderResolve::Miss));
        assert!(matches!(resolve(&['a'], 'z'), LeaderResolve::Miss));
        // A leaf consumed as a prefix is a stale prefix, not a group.
        assert!(children_of(&['q']).is_none());
    }

    #[test]
    fn whichkey_rows_list_leaves_before_groups_with_counts() {
        let rows = whichkey_rows(&[]);
        assert!(rows.iter().take_while(|row| !row.is_group).count() >= 10);
        let groups: Vec<&WhichKeyRow> = rows.iter().filter(|row| row.is_group).collect();
        // Exactly two groups at the root, with their child counts.
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, "agent");
        assert_eq!(groups[0].children, 6);
        assert_eq!(groups[1].label, "视图");
        assert_eq!(groups[1].children, 8);
        // A group prefix lists only leaves.
        let inner = whichkey_rows(&['a']);
        assert!(inner.iter().all(|row| !row.is_group));
        assert_eq!(inner.len(), 6);
        // A leaf prefix is a stale prefix — no rows.
        assert!(whichkey_rows(&['q']).is_empty());
    }

    #[test]
    fn group_children_are_reachable_through_children_of() {
        let children = children_of(&['v']).expect("v is a group");
        assert!(children.iter().any(|node| node.key() == 'd'));
    }
}
