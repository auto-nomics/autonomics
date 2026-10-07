//! StepFun (阶跃星辰) — OpenAI Chat Completions-compatible API.
//!
//! StepFun exposes its catalogue through `POST /v1/chat/completions` and uses
//! the same JSON shape as OpenAI Chat Completions, so we reuse the
//! [`OpenAiChatWire`](crate::wire::OpenAiChatWire) adapter. The Step Plan
//! (Token Plan) subscription bundle shares the same wire shape but lives on
//! a dedicated host path:
//!
//! - General API:    `https://api.stepfun.com/v1`
//! - Step Plan:      `https://api.stepfun.com/step_plan/v1`
//!
//! Authentication is Bearer-token (`STEP_API_KEY`).
//!
//! Reference: <https://platform.stepfun.com/docs/llms.txt>

use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;
use crate::types::ReasoningEffort;
use crate::wire::WireProtocolKind;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Flagship / preview
pub const MODEL_STEP_5_PREVIEW: &str = "step-5-preview";
// Step-3 flash family
pub const MODEL_STEP_3_7_FLASH: &str = "step-3.7-flash";
pub const MODEL_STEP_3_5_FLASH: &str = "step-3.5-flash";
pub const MODEL_STEP_3_5_FLASH_2603: &str = "step-3.5-flash-2603";
// Step router (Step Plan channel only)
pub const MODEL_STEP_ROUTER_V1: &str = "step-router-v1";
// Audio chat (text in, audio out)
pub const MODEL_STEPAUDIO_3_CHAT_PREVIEW: &str = "stepaudio-3-chat-preview";
pub const MODEL_STEPAUDIO_2_5_CHAT: &str = "stepaudio-2.5-chat";
// End-to-end audio models (audio in / out)
pub const MODEL_STEP_1O_AUDIO: &str = "step-1o-audio";
pub const MODEL_STEP_AUDIO_2: &str = "step-audio-2";
pub const MODEL_STEP_AUDIO_2_MINI: &str = "step-audio-2-mini";
pub const MODEL_STEP_AUDIO_R1_5: &str = "step-audio-r1.5";

/// Default base URL — general (按量计费) Chat Completions endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.stepfun.com/v1";

/// Step Plan / Token Plan subscription endpoint. Path is rewritten by the
/// provider instance; the same wire adapter appends `/v1/chat/completions`.
pub const STEP_PLAN_BASE_URL: &str = "https://api.stepfun.com/step_plan/v1";

/// Endpoint selector for the StepFun provider type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum StepfunEndpoint {
    /// General (按量计费) Chat Completions — `api.stepfun.com/v1`.
    #[default]
    Api,
    /// Step Plan / Token Plan subscription bundle — `api.stepfun.com/step_plan/v1`.
    /// Only `step-router-v1` is accepted on this endpoint (other model ids
    /// return HTTP 400 `request_params_invalid`).
    StepPlan,
}

impl StepfunEndpoint {
    pub fn base_url(self) -> &'static str {
        match self {
            StepfunEndpoint::Api => DEFAULT_BASE_URL,
            StepfunEndpoint::StepPlan => STEP_PLAN_BASE_URL,
        }
    }
}

pub struct StepfunProvider;

impl ProviderPreset for StepfunProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Stepfun
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        DEFAULT_BASE_URL
    }
    fn known_base_urls() -> Vec<&'static str> {
        // General API first, then the Step Plan (Token Plan) endpoint.
        vec![DEFAULT_BASE_URL, STEP_PLAN_BASE_URL]
    }
    fn default_auth_method() -> AuthMethod {
        // StepFun documents Bearer-token auth (`Authorization: Bearer $KEY`).
        AuthMethod::Bearer
    }
    fn wire_protocol() -> WireProtocolKind {
        // StepFun's wire shape is OpenAI Chat Completions; we reuse the
        // existing adapter, which appends `/v1/chat/completions` to the
        // configured base URL.
        WireProtocolKind::OpenaiChat
    }
}

impl StepfunProvider {
    /// Preset model catalogue for the stepfun provider type — metadata only.
    ///
    /// `provider_id` is left nil; the caller binds a model to a provider
    /// instance when persisting it.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        // Context/output caps aren't published per-model; 128K / 32K is the
        // safe ceiling across the family until vendor docs confirm otherwise.
        // Pricing isn't published either, so the catalogue carries the same
        // 0.0 placeholder the other Chinese-vendor providers use; downstream
        // billing can override per-model in the DB.
        let text_ctx: u64 = 131_072;
        let text_max: u64 = 32_768;

        vec![
            // ── Flagship preview ─────────────────────────────────────────
            // step-5-preview — flagship multimodal reasoning model.
            // Vision (`image_url`), tools, thinking via `reasoning_effort`
            // (low/medium/high).
            ModelInfoBuilder::new(MODEL_STEP_5_PREVIEW)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
            // ── Step-3 flash family ──────────────────────────────────────
            // step-3.7-flash — text fast tier, three-tier reasoning.
            ModelInfoBuilder::new(MODEL_STEP_3_7_FLASH)
                .context(text_ctx, text_max)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
            // step-3.5-flash — text fast tier, three-tier reasoning.
            ModelInfoBuilder::new(MODEL_STEP_3_5_FLASH)
                .context(text_ctx, text_max)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
            // step-3.5-flash-2603 — text fast tier, **two-tier reasoning
            // (low/high only)**. Medium isn't accepted; the SDK can't
            // currently constrain user requests, so a medium choice would
            // be rejected by the provider at request time. Defaulting
            // reasoning to `High` is still safe because high is valid.
            ModelInfoBuilder::new(MODEL_STEP_3_5_FLASH_2603)
                .context(text_ctx, text_max)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
            // ── Step router (Step Plan only) ─────────────────────────────
            // step-router-v1 — automatic routing between
            // `deepseek-v4-pro` and `step-3.7-flash`. Only callable on the
            // Step Plan endpoint; `max_tokens` capped at 250K; no image
            // or document input.
            ModelInfoBuilder::new(MODEL_STEP_ROUTER_V1)
                .context(text_ctx, 250_000)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
            // ── Audio chat models ────────────────────────────────────────
            // stepaudio-3-chat-preview — multimodal audio chat preview.
            // Audio in, audio + text out. No reasoning field.
            ModelInfoBuilder::new(MODEL_STEPAUDIO_3_CHAT_PREVIEW)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // stepaudio-2.5-chat — chat audio model, **text output only**
            // (modalities must not include audio per docs).
            ModelInfoBuilder::new(MODEL_STEPAUDIO_2_5_CHAT)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // ── End-to-end audio models (text ⇄ audio) ──────────────────
            // Each accepts `modalities: ["text","audio"]` and returns
            // audio output. `vision_ability=true` flags non-text input
            // even though the SDK's `ModelInfo` has no audio-specific
            // bit — see [`ImageSource`] / multimodal content translation.
            // step-1o-audio — first end-to-end model; voice list fetched
            // via the separate `/audio/list-voice` endpoint.
            ModelInfoBuilder::new(MODEL_STEP_1O_AUDIO)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // step-audio-2 — supports four named voices
            // (wenrounansheng / qingchunshaonv / livelybreezy-female /
            // elegantgentle-female). WAV non-streaming, PCM streaming.
            ModelInfoBuilder::new(MODEL_STEP_AUDIO_2)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // step-audio-2-mini — lighter end-to-end audio model.
            ModelInfoBuilder::new(MODEL_STEP_AUDIO_2_MINI)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // step-audio-r1.5 — reasoning-capable end-to-end audio model.
            ModelInfoBuilder::new(MODEL_STEP_AUDIO_R1_5)
                .context(text_ctx, text_max)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .max_reasoning_effort(ReasoningEffort::High)
                .pricing(0.0, 0.0)
                .build(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn endpoints_have_distinct_base_urls() {
        assert_eq!(
            StepfunEndpoint::Api.base_url(),
            "https://api.stepfun.com/v1"
        );
        assert_eq!(
            StepfunEndpoint::StepPlan.base_url(),
            "https://api.stepfun.com/step_plan/v1"
        );
    }

    #[test]
    fn default_endpoint_is_api() {
        assert_eq!(StepfunEndpoint::default(), StepfunEndpoint::Api);
    }

    #[test]
    fn known_base_urls_lists_api_first_then_step_plan() {
        let urls = StepfunProvider::known_base_urls();
        assert_eq!(urls[0], DEFAULT_BASE_URL);
        assert_eq!(urls[1], STEP_PLAN_BASE_URL);
    }

    #[test]
    fn stepfun_uses_bearer_auth_and_openai_chat_wire() {
        assert_eq!(
            <StepfunProvider as ProviderPreset>::default_auth_method(),
            AuthMethod::Bearer
        );
        assert_eq!(
            <StepfunProvider as ProviderPreset>::wire_protocol(),
            WireProtocolKind::OpenaiChat
        );
    }

    #[test]
    fn reasoning_models_advertise_max_effort_and_thinking() {
        let reasoning = [
            MODEL_STEP_5_PREVIEW,
            MODEL_STEP_3_7_FLASH,
            MODEL_STEP_3_5_FLASH,
            MODEL_STEP_3_5_FLASH_2603,
            MODEL_STEP_ROUTER_V1,
            MODEL_STEP_AUDIO_R1_5,
        ];
        for model in StepfunProvider::preset_models() {
            if reasoning.contains(&model.model_name.as_str()) {
                assert!(model.supports_thinking, "{}", model.model_name);
                assert!(model.thinking_enabled, "{}", model.model_name);
                assert_eq!(
                    model.max_reasoning_effort,
                    Some(ReasoningEffort::High),
                    "{}",
                    model.model_name
                );
            }
        }
    }

    #[test]
    fn step_plan_router_caps_max_output_at_250k() {
        let router = StepfunProvider::preset_models()
            .into_iter()
            .find(|m| m.model_name == MODEL_STEP_ROUTER_V1)
            .expect("step-router-v1 preset exists");
        assert_eq!(router.max_output_tokens, 250_000);
    }

    #[test]
    fn audio_chat_models_have_multimodal_input_flag() {
        let audio = [
            MODEL_STEPAUDIO_3_CHAT_PREVIEW,
            MODEL_STEPAUDIO_2_5_CHAT,
            MODEL_STEP_1O_AUDIO,
            MODEL_STEP_AUDIO_2,
            MODEL_STEP_AUDIO_2_MINI,
            MODEL_STEP_AUDIO_R1_5,
        ];
        for model in StepfunProvider::preset_models() {
            if audio.contains(&model.model_name.as_str()) {
                assert!(model.vision_ability, "{}", model.model_name);
            }
        }
    }

    #[test]
    fn step_5_preview_advertises_vision() {
        let model = StepfunProvider::preset_models()
            .into_iter()
            .find(|m| m.model_name == MODEL_STEP_5_PREVIEW)
            .expect("step-5-preview preset exists");
        assert!(model.vision_ability);
        assert!(model.supports_function_calling);
        assert!(model.supports_streaming);
        assert!(model.supports_thinking);
    }

    #[test]
    fn preset_models_have_nil_provider_id() {
        for model in StepfunProvider::preset_models() {
            assert_eq!(model.provider_id, Uuid::nil(), "{}", model.model_name);
        }
    }
}