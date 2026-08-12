//! `SopContext` — a stack of SOP (Standard Operating Procedure) prompts and
//! tool whitelists that the executor pushes / pops as it descends into nested
//! skills.
//!
//! ## Injection order
//!
//! When a node asks for the current system prompt or tool whitelist, the
//! stack is folded top-of-stack first:
//!
//! ```text
//! system prompt      = [workflow.sop, outer_skill.sop, ..., inner_skill.sop, node-prompt]
//! tool whitelist     = [innermost whitelist, falling back to outer, falling back to "all"]
//! ```
//!
//! The frame stack is wrapped in `Arc<parking_lot::Mutex<_>>` so the
//! [`SopGuard`] can hold its own reference and pop on `drop` without
//! borrowing `SopContext`. `parking_lot::Mutex` (sync) is used because the
//! critical sections are short and never held across `.await` points, but
//! it gives us `Send + Sync` (which `Rc<RefCell<...>>` does not).

use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::Arc;

/// A frame on the SOP stack.
#[derive(Debug, Clone)]
struct SopFrame {
    /// Prompt text contributed by this frame.
    sop: String,
    /// Optional tool whitelist; `None` means "inherit from parent".
    tools: Option<Vec<String>>,
}

/// Stack of SOP frames. Cheap to clone (`Arc`).
#[derive(Debug, Default, Clone)]
pub struct SopContext {
    inner: Arc<Mutex<Vec<SopFrame>>>,
}

impl SopContext {
    /// Empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of frames currently on the stack. Useful for assertions.
    pub fn depth(&self) -> usize {
        self.inner.lock().len()
    }

    /// Push a new SOP prompt with an optional tool whitelist.
    /// Returns a [`SopGuard`] that pops on drop — keeps push/pop balanced.
    pub fn push(&mut self, sop: String, tools: Option<Vec<String>>) -> SopGuard {
        self.inner.lock().push(SopFrame { sop, tools });
        SopGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Explicitly pop the topmost frame. Prefer RAII via [`SopGuard`].
    pub fn pop(&mut self) {
        let _ = self.inner.lock().pop();
    }

    /// Fold the entire stack into a single system prompt, top of stack first.
    pub fn system_prompt(&self) -> String {
        let frames = self.inner.lock();
        let mut parts: Vec<&str> = frames.iter().rev().map(|f| f.sop.as_str()).collect();
        parts.retain(|p| !p.is_empty());
        parts.join("\n\n")
    }

    /// Effective tool whitelist. Innermost non-empty list wins.
    pub fn tool_whitelist(&self) -> Option<Vec<String>> {
        let frames = self.inner.lock();
        for frame in frames.iter().rev() {
            if let Some(tools) = &frame.tools {
                if !tools.is_empty() {
                    return Some(tools.clone());
                }
            }
        }
        for frame in frames.iter() {
            if let Some(tools) = &frame.tools {
                if !tools.is_empty() {
                    return Some(tools.clone());
                }
            }
        }
        None
    }

    /// Returns true if `tool` is allowed by the effective whitelist.
    /// A `None` whitelist means "all tools allowed".
    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        match self.tool_whitelist() {
            Some(allowed) => allowed.iter().any(|t| t == tool),
            None => true,
        }
    }

    /// Check `tool` against the whitelist. Returns the offending tool name
    /// and the effective whitelist on failure.
    pub fn check_tool(&self, tool: &str) -> Result<(), ToolDenied> {
        let allowed: Vec<String> = self.tool_whitelist().unwrap_or_default();
        let allowed_set: HashSet<String> = allowed.iter().cloned().collect();
        if self.is_tool_allowed(tool) {
            Ok(())
        } else {
            Err(ToolDenied {
                tool: tool.to_string(),
                allowed: allowed_set,
            })
        }
    }
}

/// RAII guard that pops a [`SopContext`] frame on drop. Holds its own
/// `Arc<Mutex<...>>` so it does not borrow the context.
pub struct SopGuard {
    inner: Arc<Mutex<Vec<SopFrame>>>,
}

impl Drop for SopGuard {
    fn drop(&mut self) {
        let _ = self.inner.lock().pop();
    }
}

/// Returned by [`SopContext::check_tool`] when a tool call is denied.
#[derive(Debug, Clone)]
pub struct ToolDenied {
    /// Tool name that was denied.
    pub tool: String,
    /// Effective whitelist.
    pub allowed: HashSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stack_empty_prompt() {
        let ctx = SopContext::new();
        assert_eq!(ctx.system_prompt(), "");
        assert!(ctx.is_tool_allowed("anything"));
    }

    #[test]
    fn push_pops_on_drop() {
        let mut ctx = SopContext::new();
        {
            let _g = ctx.push("hello".into(), None);
            assert_eq!(ctx.depth(), 1);
            assert_eq!(ctx.system_prompt(), "hello");
        }
        assert_eq!(ctx.depth(), 0);
        assert_eq!(ctx.system_prompt(), "");
    }

    #[test]
    fn push_is_invocable_from_immutable_methods() {
        // The whole point of using Arc<Mutex> inside the guard is that
        // immutable methods can coexist with a held guard.
        let mut ctx = SopContext::new();
        let _g = ctx.push("hello".into(), None);
        assert_eq!(ctx.depth(), 1);
        assert_eq!(ctx.system_prompt(), "hello");
        assert!(ctx.is_tool_allowed("foo"));
    }

    #[test]
    fn top_first_injection_order() {
        let mut ctx = SopContext::new();
        let _a = ctx.push("outer".into(), None);
        let _b = ctx.push("inner".into(), None);
        assert_eq!(ctx.system_prompt(), "inner\n\nouter");
    }

    #[test]
    fn empty_frames_omitted() {
        let mut ctx = SopContext::new();
        let _a = ctx.push("".into(), None);
        let _b = ctx.push("only".into(), None);
        assert_eq!(ctx.system_prompt(), "only");
    }

    #[test]
    fn whitelist_innermost_wins() {
        let mut ctx = SopContext::new();
        let _a = ctx.push("outer".into(), Some(vec!["a".into(), "b".into()]));
        let _b = ctx.push("inner".into(), Some(vec!["c".into()]));
        assert!(ctx.is_tool_allowed("c"));
        assert!(!ctx.is_tool_allowed("a"));
    }

    #[test]
    fn whitelist_empty_inner_falls_through() {
        let mut ctx = SopContext::new();
        let _a = ctx.push("outer".into(), Some(vec!["a".into()]));
        let _b = ctx.push("inner".into(), Some(vec![]));
        assert!(ctx.is_tool_allowed("a"));
    }

    #[test]
    fn check_tool_owns_data() {
        let mut ctx = SopContext::new();
        let _g = ctx.push("outer".into(), Some(vec!["a".into()]));
        let err = ctx.check_tool("zzz").unwrap_err();
        assert_eq!(err.tool, "zzz");
        assert!(err.allowed.contains("a"));
        assert!(ctx.check_tool("a").is_ok());
    }
}
