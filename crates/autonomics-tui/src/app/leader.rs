//! LazyVim-style leader keys: `<Space>` enters a pending-prefix state, the
//! next key (or key sequence, for groups) resolves to an action, and a
//! which-key hint strip shows what the current prefix offers.
//!
//! Actions reuse the command palette's [`CommandAction`] verbatim, so a
//! leader binding and its palette entry can never drift apart.

use super::*;
use i18n::t;
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
}

/// The root menu. `<Space><Space>` opens the palette, mirroring LazyVim's
/// `<leader><leader>` smart-find slot; groups cluster the rest.
pub(super) const LEADER_ROOT: &[LeaderNode] = &[
    LeaderNode::Leaf {
        key: '|',
        label: "tui.leader.splitRight",
        action: CommandAction::SplitVertical,
    },
    LeaderNode::Group {
        key: 'w',
        label: "tui.leader.window",
        children: &[
            LeaderNode::Leaf {
                key: 'v',
                label: "tui.leader.splitRight",
                action: CommandAction::SplitVertical,
            },
            LeaderNode::Leaf {
                key: 's',
                label: "tui.leader.splitBelow",
                action: CommandAction::SplitHorizontal,
            },
            LeaderNode::Leaf {
                key: 'w',
                label: "tui.leader.focusOther",
                action: CommandAction::FocusOtherSplit,
            },
            LeaderNode::Leaf {
                key: 'd',
                label: "tui.leader.closeSplit",
                action: CommandAction::CloseSplit,
            },
        ],
    },
    LeaderNode::Leaf {
        key: ' ',
        label: "tui.leader.commandPalette",
        action: CommandAction::OpenCommandPalette,
    },
    LeaderNode::Leaf {
        key: 'q',
        label: "tui.leader.quit",
        action: CommandAction::Quit,
    },
    LeaderNode::Group {
        key: 'a',
        label: "tui.leader.agent",
        children: &[
            LeaderNode::Leaf {
                key: 'n',
                label: "tui.leader.newAgent",
                action: CommandAction::NewAgent,
            },
            LeaderNode::Leaf {
                key: 'r',
                label: "tui.leader.resume",
                action: CommandAction::ResumeAgent,
            },
            LeaderNode::Leaf {
                key: 't',
                label: "tui.leader.switcher",
                action: CommandAction::FocusAgentsPanel,
            },
            LeaderNode::Leaf {
                key: 'c',
                label: "tui.leader.close",
                action: CommandAction::CloseAgent,
            },
            LeaderNode::Leaf {
                key: 'd',
                label: "tui.leader.delete",
                action: CommandAction::DeleteAgent,
            },
            LeaderNode::Leaf {
                key: 'x',
                label: "tui.leader.cancel",
                action: CommandAction::CancelAgent,
            },
        ],
    },
    LeaderNode::Group {
        key: 'v',
        label: "tui.leader.view",
        children: &[
            LeaderNode::Leaf {
                key: 'd',
                label: "tui.leader.dag",
                action: CommandAction::ViewDag,
            },
            LeaderNode::Leaf {
                key: 'b',
                label: "tui.leader.bibliography",
                action: CommandAction::BibliographyPanel,
            },
            LeaderNode::Leaf {
                key: 't',
                label: "tui.leader.chatOnlyView",
                action: CommandAction::ToggleChatOnly,
            },
            LeaderNode::Leaf {
                key: 'h',
                label: "tui.leader.collapseThinking",
                action: CommandAction::ToggleCollapseThinking,
            },
            LeaderNode::Leaf {
                key: 'c',
                label: "tui.leader.collapseToolCalls",
                action: CommandAction::ToggleCollapseToolCalls,
            },
            LeaderNode::Leaf {
                key: 'r',
                label: "tui.leader.collapseToolResults",
                action: CommandAction::ToggleCollapseToolResults,
            },
        ],
    },
    LeaderNode::Leaf {
        key: 'm',
        label: "tui.leader.modelConfig",
        action: CommandAction::ModelConfig,
    },
    LeaderNode::Leaf {
        key: 'P',
        label: "tui.leader.computeDashboard",
        action: CommandAction::HpcPanel,
    },
    LeaderNode::Leaf {
        key: 'e',
        label: "tui.leader.embeddingConfig",
        action: CommandAction::EmbeddingConfig,
    },
    LeaderNode::Leaf {
        key: 'g',
        label: "tui.leader.agentConfig",
        action: CommandAction::AgentConfig,
    },
    LeaderNode::Leaf {
        key: 'h',
        label: "tui.leader.historySearch",
        action: CommandAction::HistorySearch,
    },
    LeaderNode::Leaf {
        key: 'y',
        label: "tui.leader.copyMessage",
        action: CommandAction::CopyMessage,
    },
    LeaderNode::Leaf {
        key: 'C',
        label: "tui.leader.compactContext",
        action: CommandAction::Compact,
    },
    LeaderNode::Leaf {
        key: 'G',
        label: "tui.leader.followOutput",
        action: CommandAction::ToggleAutoScroll,
    },
    LeaderNode::Leaf {
        key: 'p',
        label: "tui.leader.planPopup",
        action: CommandAction::OpenPlanPopup,
    },
    LeaderNode::Leaf {
        key: 'T',
        label: "tui.leader.tasksPopup",
        action: CommandAction::OpenTasksPopup,
    },
    LeaderNode::Leaf {
        key: 'u',
        label: "tui.leader.toggleTheme",
        action: CommandAction::CycleTheme,
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
    pub label: std::borrow::Cow<'static, str>,
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
                label: t!(label),
                is_group: false,
                children: 0,
            }),
            LeaderNode::Group {
                key,
                label,
                children,
            } => rows.push(WhichKeyRow {
                key: *key,
                label: t!(label),
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
        // Exactly three groups at the root, with their child counts.
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].label, "window");
        assert_eq!(groups[0].children, 4);
        assert_eq!(groups[1].label, "agent");
        assert_eq!(groups[1].children, 6);
        assert_eq!(groups[2].label, "view");
        assert_eq!(groups[2].children, 6);
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
