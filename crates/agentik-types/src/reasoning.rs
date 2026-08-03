//! Reasoning / thinking configuration.
//!
//! The Anthropic Messages API accepts `thinking: { type: "enabled",
//! budget_tokens: N }`; OpenAI reasoning models accept either top-level
//! `reasoning_effort: "low" | "medium" | "high"` (Chat Completions) or
//! `reasoning: { effort: ... }` (Responses API).
//!
//! To keep the SDK's public surface protocol-neutral we expose both styles on
//! [`crate::messages::MessageCreateParams`] and let each `WireProtocol` impl
//! pick the right one for its endpoint, translating or degrading as advertised
//! via its `ProtocolFeatures`.

use serde::{Deserialize, Serialize};

/// Anthropic-style thinking configuration.
///
/// When attached to a [`crate::messages::MessageCreateParams`] request, this
/// serialises directly to the Anthropic Messages API `thinking` field. Other
/// wire protocols translate it as best they can — see each `WireProtocol`'s
/// `features()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// `"enabled"` to turn extended thinking on; `"disabled"` to turn it off.
    #[serde(rename = "type")]
    pub kind: ThinkingKind,
    /// Token budget for the thinking phase. Required when `kind = Enabled`.
    /// Ignored when `kind = Disabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

/// Tag for [`ThinkingConfig::kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingKind {
    /// Enable extended thinking.
    Enabled,
    /// Disable extended thinking (overrides a model's default).
    Disabled,
}

impl ThinkingConfig {
    /// Enable thinking with a token budget.
    #[must_use]
    pub fn enabled(budget_tokens: u32) -> Self {
        Self {
            kind: ThinkingKind::Enabled,
            budget_tokens: Some(budget_tokens),
        }
    }

    /// Disable thinking on a model that has it on by default.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            kind: ThinkingKind::Disabled,
            budget_tokens: None,
        }
    }
}

/// Protocol-neutral reasoning configuration.
///
/// Carries whichever of the two wire styles the caller picked; the active
/// `WireProtocol` is responsible for emitting (or down-translating) the
/// appropriate field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReasoningConfig {
    /// Token-budget style (Anthropic / GLM "thinking").
    Budget {
        #[serde(rename = "budget_tokens")]
        budget_tokens: u32,
    },
    /// Effort-level style (OpenAI and most other providers).
    Effort(ReasoningEffort),
}

/// Effort-level reasoning intensity.
///
/// Variants follow the OpenAI Responses API vocabulary
/// (`none | minimal | low | medium | high | xhigh | max | ultra`); values that
/// a given endpoint doesn't recognise are dropped on the wire.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// No reasoning.
    None,
    /// Bare-minimum reasoning.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort (the typical default).
    #[default]
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    Xhigh,
    /// Maximum reasoning effort.
    Max,
    /// Beyond-maximum reasoning effort (provider-defined).
    Ultra,
}

impl ReasoningConfig {
    #[must_use]
    pub fn from_budget(budget_tokens: u32) -> Self {
        Self::Budget { budget_tokens }
    }

    #[must_use]
    pub fn from_effort(effort: ReasoningEffort) -> Self {
        Self::Effort(effort)
    }
}

/// Effort → heuristic Anthropic thinking budget, used when an Anthropic-only
/// wire receives an effort value. The exact mapping is provider-tunable;
/// these defaults track observed Claude Sonnet 5 ranges.
///
/// Returns `None` for [`ReasoningEffort::None`] / [`ReasoningEffort::Minimal`]
/// so the wire can drop the field rather than emit a zero-budget block.
#[must_use]
pub fn anthropic_budget_for_effort(effort: ReasoningEffort) -> Option<u32> {
    match effort {
        ReasoningEffort::None | ReasoningEffort::Minimal => None,
        ReasoningEffort::Low => Some(2_048),
        ReasoningEffort::Medium => Some(8_192),
        ReasoningEffort::High => Some(16_384),
        ReasoningEffort::Xhigh => Some(24_576),
        ReasoningEffort::Max | ReasoningEffort::Ultra => Some(32_768),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_enabled_serialises_anthropic_shape() {
        let t = ThinkingConfig::enabled(4_096);
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["type"], "enabled");
        assert_eq!(v["budget_tokens"], 4_096);
    }

    #[test]
    fn thinking_disabled_omits_budget() {
        let t = ThinkingConfig::disabled();
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["type"], "disabled");
        assert!(v.get("budget_tokens").is_none());
    }

    #[test]
    fn reasoning_effort_serialises_lowercase() {
        let v = serde_json::to_value(ReasoningEffort::Xhigh).unwrap();
        assert_eq!(v, "xhigh");
    }

    #[test]
    fn reasoning_config_untagged_budget_vs_effort() {
        let budget = ReasoningConfig::from_budget(1_000);
        let v = serde_json::to_value(&budget).unwrap();
        assert_eq!(v["budget_tokens"], 1_000);

        let effort = ReasoningConfig::from_effort(ReasoningEffort::High);
        let v = serde_json::to_value(&effort).unwrap();
        // Untagged Effort variant serialises as the inner enum value.
        assert_eq!(v, "high");
    }

    #[test]
    fn budget_for_effort_default_ranges() {
        assert_eq!(anthropic_budget_for_effort(ReasoningEffort::None), None);
        assert_eq!(
            anthropic_budget_for_effort(ReasoningEffort::Medium),
            Some(8_192)
        );
        assert_eq!(
            anthropic_budget_for_effort(ReasoningEffort::Ultra),
            Some(32_768)
        );
    }
}
