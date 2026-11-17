//! Hierarchy policy: which inter-agent operations a caller may perform.
//! Delegation and management (shutdown / interrupt) both reach only the
//! caller's direct children.

use super::super::RuntimeHost;
use super::resolution::path_is_ancestor;

impl RuntimeHost {
    /// Validate delegation hierarchy: direct children only.
    ///
    /// A direct child is always delegable regardless of its live status —
    /// messages queue on the child's relay and are processed turn by turn.
    /// On success returns the validated, parsed target path.
    pub(super) fn validate_delegation_paths(
        &self,
        caller_path: &str,
        target_path: &str,
    ) -> std::result::Result<agentik_types::AgentPath, String> {
        let caller = self.resolve_live_agent(caller_path, "caller", caller_path)?;
        // The TARGET may be dormant (virtual-actor activation brings it
        // up on delivery); hierarchy is validated on the paths alone.
        let target = self
            .resolve_agent(target_path)
            .unwrap_or_else(|| target_path.to_string());
        let Ok(caller_path) = agentik_types::AgentPath::try_from(caller.as_str()) else {
            return Err(format!("invalid caller path `{caller}`"));
        };
        let Ok(target_path) = agentik_types::AgentPath::try_from(target.as_str()) else {
            return Err(format!("invalid delegate target path `{target}`"));
        };
        if caller_path == target_path {
            return Err("Agents cannot delegate tasks to themselves.".to_string());
        }
        if target_path.is_direct_child_of(&caller_path) {
            return Ok(target_path);
        }
        if path_is_ancestor(&target_path, &caller_path) {
            return Err(format!(
                "Delegation denied: '{}' is a superior of '{}'. \
                 Subordinate agents cannot delegate tasks upward.",
                target_path.as_str(),
                caller_path.as_str()
            ));
        }
        Err(format!(
            "Delegation denied: '{}' is not a direct child of '{}'. \
             Delegation is limited to your own direct children; siblings and other \
             branches are not visible to you. Spawn the agent you need with \
             spawn_agent, or reuse an existing child.",
            target_path.as_str(),
            caller_path.as_str()
        ))
    }

    /// Validate that `caller_path` may manage (shut down / interrupt) the
    /// agent at `target`: agents control only their own direct children.
    pub(super) fn validate_child_control(
        &self,
        caller_path: &str,
        target: &str,
        action: &str,
    ) -> std::result::Result<(), String> {
        let caller = self.resolve_live_agent(caller_path, "caller", caller_path)?;
        let Ok(caller) = agentik_types::AgentPath::try_from(caller.as_str()) else {
            return Err(format!("invalid caller path `{caller}`"));
        };
        let Ok(target) = agentik_types::AgentPath::try_from(target) else {
            return Err(format!("invalid target path `{target}`"));
        };
        if target.is_direct_child_of(&caller) {
            return Ok(());
        }
        Err(format!(
            "Control denied: '{}' is not a direct child of '{}'. \
             Agents may only {action} their own direct children.",
            target.as_str(),
            caller.as_str()
        ))
    }
}

#[cfg(test)]
mod policy_tests {
    //! Hierarchy invariants: delegation and management reach only the
    //! caller's direct children; the operator path stays unrestricted.

    use super::super::test_support::{config, register_agent_at, set_status};
    use crate::control::HostCommand;
    use crate::host::RuntimeHost;

    /// Delegation reaches only the caller's direct children. Siblings,
    /// cousins, grandparents, grandchildren, and self are all rejected;
    /// a busy child stays delegable (messages queue on its relay).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delegation_is_restricted_to_direct_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: std::sync::Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>> =
            std::sync::Arc::new(arc_swap::ArcSwapOption::from_pointee(None));
        for path in [
            "/root/a",
            "/root/b",
            "/root/a/child",
            "/root/b/child",
            "/root/a/child/grand",
        ] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        // Parent → direct child is the one allowed direction.
        assert!(
            host.validate_delegation_paths("/root/a", "/root/a/child")
                .is_ok()
        );
        // Busy children remain delegable — turns queue on the relay.
        set_status(
            &mut host,
            "/root/a/child",
            agentik_types::AgentLifecycleStatus::Requesting,
        );
        assert!(
            host.validate_delegation_paths("/root/a", "/root/a/child")
                .is_ok()
        );
        set_status(
            &mut host,
            "/root/a/child",
            agentik_types::AgentLifecycleStatus::Idle,
        );

        // Upward delegation is rejected with the superior-specific message.
        let upward = host
            .validate_delegation_paths("/root/a/child", "/root/a")
            .unwrap_err();
        assert!(upward.contains("superior"), "{upward}");

        // Siblings, cousins, and grandchildren are not direct children.
        let sibling = host
            .validate_delegation_paths("/root/a", "/root/b")
            .unwrap_err();
        assert!(sibling.contains("not a direct child"), "{sibling}");
        let cousin = host
            .validate_delegation_paths("/root/a/child", "/root/b/child")
            .unwrap_err();
        assert!(cousin.contains("not a direct child"), "{cousin}");
        let grandchild = host
            .validate_delegation_paths("/root/a", "/root/a/child/grand")
            .unwrap_err();
        assert!(grandchild.contains("not a direct child"), "{grandchild}");

        // Self-delegation is rejected.
        let self_delegate = host
            .validate_delegation_paths("/root/a", "/root/a")
            .unwrap_err();
        assert!(self_delegate.contains("themselves"), "{self_delegate}");
    }

    /// Agent-initiated shutdown/interrupt reach only the caller's direct
    /// children; the operator (caller_path = None) path stays unrestricted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_and_interrupt_are_child_scoped_for_agents() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: std::sync::Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>> =
            std::sync::Arc::new(arc_swap::ArcSwapOption::from_pointee(None));
        for path in ["/root/a", "/root/b", "/root/a/child"] {
            register_agent_at(&mut host, path, model.clone()).await;
        }

        // Agent trying to shut down a non-child: denied, target survives.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(HostCommand::Shutdown {
            name: "/root/b".into(),
            caller_path: Some("/root/a".into()),
            evict: false,
            reply_tx: Some(tx),
        });
        let denied = rx.await.unwrap();
        let message =
            denied.expect_err("shutdown of a non-child must be denied by an agent caller");
        assert!(message.contains("direct child"), "{message}");
        assert!(host.agents.contains_key("/root/b"));

        // Agent interrupting a non-child: denied the same way.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(HostCommand::CancelAgent {
            name: "/root/b".into(),
            caller_path: Some("/root/a".into()),
            reply_tx: Some(tx),
        });
        let denied = rx.await.unwrap();
        let message =
            denied.expect_err("interrupt of a non-child must be denied by an agent caller");
        assert!(message.contains("direct child"), "{message}");
        assert!(host.agents.contains_key("/root/b"));

        // Agent shutting down its own direct child: allowed.
        let (tx, rx) = tokio::sync::oneshot::channel();
        host.process_command(HostCommand::Shutdown {
            name: "/root/a/child".into(),
            caller_path: Some("/root/a".into()),
            evict: false,
            reply_tx: Some(tx),
        });
        rx.await.unwrap().unwrap();
        assert!(!host.agents.contains_key("/root/a/child"));

        // Operator (None caller) shutting down any agent: unrestricted.
        host.process_command(HostCommand::Shutdown {
            name: "/root/b".into(),
            caller_path: None,
            evict: false,
            reply_tx: None,
        });
        assert!(!host.agents.contains_key("/root/b"));
    }
}
