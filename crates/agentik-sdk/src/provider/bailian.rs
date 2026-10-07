//! Alibaba Cloud Bailian (Model Studio / 百炼) — Anthropic Messages API.
//!
//! Bailian speaks the Anthropic Messages protocol on top of regional gateways,
//! plus two dedicated Coding Plan (Token Plan) domains for the subscription
//! bundles (`sk-sp-…` API keys; isolated from the regular `sk-…` keys).
//!
//! All preset URLs embed a `<WORKSPACE-ID>` subdomain placeholder; the user
//! replaces it with their workspace ID in the TUI base-url editor before the
//! provider is usable. The Coding Plan URLs are workspace-independent.
//!
//! Reference: <https://docs.bailian.console.aliyun.com/zh/model-studio/anthropic-api-messages>

use crate::http::auth::AuthMethod;
use crate::model::ModelInfo;
use crate::model::ProviderType;
use crate::model::model_info::ModelInfoBuilder;
use crate::provider::ProviderPreset;

// ─── Model IDs ──────────────────────────────────────────────────────────────
// Qwen Max — flagship reasoning
pub const MODEL_QWEN3_8_MAX: &str = "qwen3.8-max";
pub const MODEL_QWEN3_8_MAX_0902: &str = "qwen3.8-max-0902";
pub const MODEL_QWEN3_7_MAX: &str = "qwen3.7-max";
pub const MODEL_QWEN3_7_MAX_2026_05_20: &str = "qwen3.7-max-2026-05-20";
pub const MODEL_QWEN3_7_MAX_2026_06_08: &str = "qwen3.7-max-2026-06-08";
pub const MODEL_QWEN3_6_MAX_PREVIEW: &str = "qwen3.6-max-preview";
pub const MODEL_QWEN3_MAX: &str = "qwen3-max";
pub const MODEL_QWEN3_MAX_2026_01_23: &str = "qwen3-max-2026-01-23";
pub const MODEL_QWEN3_MAX_PREVIEW: &str = "qwen3-max-preview";
// Qwen Plus — high-throughput
pub const MODEL_QWEN3_7_PLUS: &str = "qwen3.7-plus";
pub const MODEL_QWEN3_7_PLUS_2026_05_26: &str = "qwen3.7-plus-2026-05-26";
pub const MODEL_QWEN3_6_PLUS: &str = "qwen3.6-plus";
pub const MODEL_QWEN3_6_PLUS_2026_04_02: &str = "qwen3.6-plus-2026-04-02";
pub const MODEL_QWEN3_5_PLUS: &str = "qwen3.5-plus";
pub const MODEL_QWEN3_5_PLUS_2026_04_20: &str = "qwen3.5-plus-2026-04-20";
pub const MODEL_QWEN3_5_PLUS_2026_02_15: &str = "qwen3.5-plus-2026-02-15";
pub const MODEL_QWEN_PLUS: &str = "qwen-plus";
pub const MODEL_QWEN_PLUS_LATEST: &str = "qwen-plus-latest";
pub const MODEL_QWEN_PLUS_2025_09_11: &str = "qwen-plus-2025-09-11";
// Qwen Flash — fast, low-cost
pub const MODEL_QWEN3_8_FLASH: &str = "qwen3.8-flash";
pub const MODEL_QWEN3_7_FLASH: &str = "qwen3.7-flash";
pub const MODEL_QWEN3_7_FLASH_2026_07_15: &str = "qwen3.7-flash-2026-07-15";
pub const MODEL_QWEN3_6_FLASH: &str = "qwen3.6-flash";
pub const MODEL_QWEN3_6_FLASH_2026_04_16: &str = "qwen3.6-flash-2026-04-16";
pub const MODEL_QWEN3_5_FLASH: &str = "qwen3.5-flash";
pub const MODEL_QWEN3_5_FLASH_2026_02_23: &str = "qwen3.5-flash-2026-02-23";
pub const MODEL_QWEN_FLASH: &str = "qwen-flash";
pub const MODEL_QWEN_FLASH_2025_07_28: &str = "qwen-flash-2025-07-28";
// Qwen Turbo — legacy fast
pub const MODEL_QWEN_TURBO: &str = "qwen-turbo";
// Qwen Coder — code-specialised
pub const MODEL_QWEN3_CODER_NEXT: &str = "qwen3-coder-next";
pub const MODEL_QWEN3_CODER_PLUS: &str = "qwen3-coder-plus";
pub const MODEL_QWEN3_CODER_PLUS_2025_09_23: &str = "qwen3-coder-plus-2025-09-23";
pub const MODEL_QWEN3_CODER_FLASH: &str = "qwen3-coder-flash";
// Qwen VL — vision
pub const MODEL_QWEN3_VL_PLUS: &str = "qwen3-vl-plus";
pub const MODEL_QWEN3_VL_FLASH: &str = "qwen3-vl-flash";
pub const MODEL_QWEN_VL_MAX: &str = "qwen-vl-max";
pub const MODEL_QWEN_VL_PLUS: &str = "qwen-vl-plus";
// Qwen open-source checkpoints
pub const MODEL_QWEN3_6_27B: &str = "qwen3.6-27b";
pub const MODEL_QWEN3_5_397B_A17B: &str = "qwen3.5-397b-a17b";
pub const MODEL_QWEN3_5_122B_A10B: &str = "qwen3.5-122b-a10b";
pub const MODEL_QWEN3_5_27B: &str = "qwen3.5-27b";
pub const MODEL_QWEN3_5_35B_A3B: &str = "qwen3.5-35b-a3b";
pub const MODEL_QWEN3_8_2_4T_A95B: &str = "qwen3.8-2.4t-a95b";
pub const MODEL_QWEN3_8_27B: &str = "qwen3.8-27b";
// Third-party models
pub const MODEL_DEEPSEEK_V4_PRO: &str = "deepseek-v4-pro";
pub const MODEL_DEEPSEEK_V4_PRO_0813: &str = "deepseek-v4-pro-0813";
pub const MODEL_DEEPSEEK_V4_FLASH: &str = "deepseek-v4-flash";
pub const MODEL_DEEPSEEK_V4_FLASH_0731: &str = "deepseek-v4-flash-0731";
pub const MODEL_DEEPSEEK_V4_1_FLASH: &str = "deepseek-v4.1-flash";
pub const MODEL_DEEPSEEK_V3_2: &str = "deepseek-v3.2";
pub const MODEL_KIMI_K3: &str = "kimi-k3";
pub const MODEL_KIMI_K2_7_CODE: &str = "kimi-k2.7-code";
pub const MODEL_KIMI_K2_6: &str = "kimi-k2.6";
pub const MODEL_KIMI_K2_5: &str = "kimi-k2.5";
pub const MODEL_KIMI_K2_THINKING: &str = "kimi-k2-thinking";
pub const MODEL_GLM_5_3: &str = "glm-5.3";
pub const MODEL_GLM_5_2: &str = "glm-5.2";
pub const MODEL_GLM_5_1: &str = "glm-5.1";
pub const MODEL_GLM_5: &str = "glm-5";
pub const MODEL_GLM_4_7: &str = "glm-4.7";
pub const MODEL_GLM_4_6: &str = "glm-4.6";
pub const MODEL_MINIMAX_M2_5: &str = "MiniMax-M2.5";
pub const MODEL_MINIMAX_M2_1: &str = "MiniMax-M2.1";

/// Workspace-id subdomain placeholder embedded in the preset URL.
///
/// Users replace it (in the provider-config panel) before the preset is functional.
const WORKSPACE_PLACEHOLDER: &str = "<WORKSPACE-ID>";

/// Bailian general regional gateway (按量计费 / pay-as-you-go).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BailianRegion {
    /// 华北2（北京）
    #[default]
    Beijing,
    /// 新加坡
    Singapore,
    /// 美国（弗吉尼亚）
    UsVirginia,
    /// 德国（法兰克福）
    GermanyFrankfurt,
    /// 日本（东京）
    JapanTokyo,
}

impl BailianRegion {
    /// Host portion of the regional gateway. The full URL is
    /// `https://<workspace>.<host>/apps/anthropic`; the workspace is a
    /// user-supplied subdomain not enumerated here.
    pub fn host(self) -> &'static str {
        match self {
            BailianRegion::Beijing => "cn-beijing.maas.aliyuncs.com",
            BailianRegion::Singapore => "ap-southeast-1.maas.aliyuncs.com",
            BailianRegion::UsVirginia => "us-east-1.maas.aliyuncs.com",
            BailianRegion::GermanyFrankfurt => "eu-central-1.maas.aliyuncs.com",
            BailianRegion::JapanTokyo => "ap-northeast-1.maas.aliyuncs.com",
        }
    }

    /// Render the full Anthropic-Messages base URL with the workspace-id
    /// placeholder inserted.
    pub fn base_url(self) -> String {
        format!(
            "https://{WORKSPACE_PLACEHOLDER}.{}/apps/anthropic",
            self.host()
        )
    }
}

/// Coding Plan / Token Plan regional split.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BailianTokenPlanRegion {
    /// 国内版：`https://coding.dashscope.aliyuncs.com/apps/anthropic`
    #[default]
    Domestic,
    /// 国际版：`https://coding-intl.dashscope.aliyuncs.com/apps/anthropic`
    International,
}

impl BailianTokenPlanRegion {
    pub fn base_url(self) -> &'static str {
        match self {
            BailianTokenPlanRegion::Domestic => {
                "https://coding.dashscope.aliyuncs.com/apps/anthropic"
            }
            BailianTokenPlanRegion::International => {
                "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
            }
        }
    }
}

/// Endpoint selector for the Bailian provider type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BailianEndpoint {
    /// General (按量计费) regional gateway — `{region}.maas.aliyuncs.com`.
    Main(BailianRegion),
    /// Coding Plan / Token Plan subscription bundle. API keys carry the
    /// `sk-sp-…` prefix and **must not** be mixed with regular `sk-…` keys.
    TokenPlan(BailianTokenPlanRegion),
}

impl Default for BailianEndpoint {
    fn default() -> Self {
        BailianEndpoint::Main(BailianRegion::default())
    }
}

impl BailianEndpoint {
    /// Default endpoint used when the user has not yet picked one. Defaults
    /// to the Beijing regional gateway (Aliyun's home region).
    pub fn base_url(self) -> String {
        match self {
            BailianEndpoint::Main(region) => region.base_url(),
            BailianEndpoint::TokenPlan(region) => region.base_url().to_string(),
        }
    }

    /// Stable token for this endpoint, suitable as a `known_base_urls` entry.
    pub fn as_static_str(self) -> &'static str {
        match self {
            BailianEndpoint::Main(BailianRegion::Beijing) => {
                "https://<WORKSPACE-ID>.cn-beijing.maas.aliyuncs.com/apps/anthropic"
            }
            BailianEndpoint::TokenPlan(BailianTokenPlanRegion::Domestic) => {
                "https://coding.dashscope.aliyuncs.com/apps/anthropic"
            }
            BailianEndpoint::TokenPlan(BailianTokenPlanRegion::International) => {
                "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
            }
            // Other regional hosts aren't in the known_base_urls shortlist;
            // they remain reachable through custom URL editing in the UI.
            _ => "https://<WORKSPACE-ID>.cn-beijing.maas.aliyuncs.com/apps/anthropic",
        }
    }
}

pub struct BailianProvider;

impl ProviderPreset for BailianProvider {
    fn provider_type() -> ProviderType {
        ProviderType::Bailian
    }
    fn preset_models() -> Vec<ModelInfo> {
        Self::model_definitions()
    }
    fn default_base_url() -> &'static str {
        // Beijing regional gateway with the workspace-id placeholder — the
        // user must replace `<WORKSPACE-ID>` with their actual workspace id.
        "https://<WORKSPACE-ID>.cn-beijing.maas.aliyuncs.com/apps/anthropic"
    }
    fn known_base_urls() -> Vec<&'static str> {
        // Most common pair: the Beijing regional gateway (general) and the
        // Coding Plan / Token Plan domains (subscription bundle). The TUI
        // surfaces these as base-url cycle options.
        vec![
            BailianEndpoint::Main(BailianRegion::Beijing).as_static_str(),
            BailianEndpoint::TokenPlan(BailianTokenPlanRegion::Domestic).as_static_str(),
            BailianEndpoint::TokenPlan(BailianTokenPlanRegion::International).as_static_str(),
        ]
    }
    fn default_auth_method() -> AuthMethod {
        // Bailian accepts either `x-api-key` (Anthropic) or
        // `Authorization: Bearer`. The Anthropic header is the documented
        // primary path; users may switch to Bearer via the auth-method
        // editor if their gateway setup demands it.
        AuthMethod::Anthropic
    }
}

impl BailianProvider {
    /// Preset model catalogue for the bailian provider type — metadata only.
    ///
    /// `provider_id` is left nil; the caller binds a model to a provider
    /// instance when persisting it.
    pub fn preset_models() -> Vec<ModelInfo> {
        <Self as ProviderPreset>::preset_models()
    }

    fn model_definitions() -> Vec<ModelInfo> {
        vec![
            // ── Qwen Max — flagship reasoning ────────────────────────────
            // Default thinking ON for qwen3.8-max. All entries share
            // 256K context / 32K output until vendor docs confirm otherwise.
            ModelInfoBuilder::new(MODEL_QWEN3_8_MAX)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_8_MAX_0902)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_MAX)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_MAX_2026_05_20)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_MAX_2026_06_08)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_MAX_PREVIEW)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_MAX)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_MAX_2026_01_23)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_MAX_PREVIEW)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen Plus — high-throughput mid-tier ──────────────────────
            ModelInfoBuilder::new(MODEL_QWEN3_7_PLUS)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_PLUS_2026_05_26)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_PLUS)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_PLUS_2026_04_02)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_PLUS)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_PLUS_2026_04_20)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_PLUS_2026_02_15)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_PLUS)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_PLUS_LATEST)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_PLUS_2025_09_11)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen Flash — fast, low-cost ───────────────────────────────
            // qwen3.8-flash defaults thinking ON per docs.
            ModelInfoBuilder::new(MODEL_QWEN3_8_FLASH)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_FLASH)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_7_FLASH_2026_07_15)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_FLASH)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_FLASH_2026_04_16)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_FLASH)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_FLASH_2026_02_23)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_FLASH)
                .context(131_072, 32_768)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_FLASH_2025_07_28)
                .context(131_072, 32_768)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen Turbo — legacy fast tier ─────────────────────────────
            ModelInfoBuilder::new(MODEL_QWEN_TURBO)
                .context(131_072, 32_768)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen Coder — code-specialised, thinking default ON ────────
            ModelInfoBuilder::new(MODEL_QWEN3_CODER_NEXT)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_CODER_PLUS)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_CODER_PLUS_2025_09_23)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_CODER_FLASH)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen VL — vision-capable ──────────────────────────────────
            ModelInfoBuilder::new(MODEL_QWEN3_VL_PLUS)
                .context(131_072, 32_768)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_VL_FLASH)
                .context(131_072, 32_768)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_VL_MAX)
                .context(32_768, 8_192)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN_VL_PLUS)
                .context(32_768, 8_192)
                .capabilities(true, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // ── Qwen open-source checkpoints (MoE / dense) ────────────────
            // Sizing hints come from model-name totals (e.g. `397b-a17b` =
            // 397B total / 17B active). Context/output default to the
            // typical Qwen3 instruction-tuned window.
            ModelInfoBuilder::new(MODEL_QWEN3_8_2_4T_A95B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_8_27B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_6_27B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_397B_A17B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_122B_A10B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_35B_A3B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_QWEN3_5_27B)
                .context(131_072, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // ── Third-party: DeepSeek v4 series ───────────────────────────
            // Thinking default ON for all v4 entries per docs.
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_PRO)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_PRO_0813)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_FLASH)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_FLASH_0731)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V4_1_FLASH)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_DEEPSEEK_V3_2)
                .context(131_072, 32_768)
                .capabilities(false, true, true, false)
                .pricing(0.0, 0.0)
                .build(),
            // ── Third-party: Kimi / Moonshot ──────────────────────────────
            // kimi-k3 — flagship vision + thinking.
            ModelInfoBuilder::new(MODEL_KIMI_K3)
                .context(1_048_576, 32_768)
                .capabilities(true, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // kimi-k2.7-code — only supports thinking (cannot disable).
            ModelInfoBuilder::new(MODEL_KIMI_K2_7_CODE)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_required()
                .pricing(0.0, 0.0)
                .build(),
            // kimi-k2.6 — thinking default OFF per docs.
            ModelInfoBuilder::new(MODEL_KIMI_K2_6)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // kimi-k2.5 — thinking default OFF per docs.
            ModelInfoBuilder::new(MODEL_KIMI_K2_5)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // kimi-k2-thinking — only supports thinking (cannot disable).
            ModelInfoBuilder::new(MODEL_KIMI_K2_THINKING)
                .context(262_144, 32_768)
                .capabilities(false, true, true, true)
                .thinking_required()
                .pricing(0.0, 0.0)
                .build(),
            // ── Third-party: GLM series ───────────────────────────────────
            // All GLM entries default thinking ON per docs.
            ModelInfoBuilder::new(MODEL_GLM_5_3)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_5_2)
                .context(1_048_576, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_5_1)
                .context(200_000, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_5)
                .context(200_000, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_4_7)
                .context(200_000, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_GLM_4_6)
                .context(200_000, 32_768)
                .capabilities(false, true, true, true)
                .thinking_enabled(None)
                .pricing(0.0, 0.0)
                .build(),
            // ── Third-party: MiniMax M2 series ────────────────────────────
            // Both entries only support thinking per docs.
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_5)
                .context(204_800, 32_768)
                .capabilities(false, true, true, true)
                .thinking_required()
                .pricing(0.0, 0.0)
                .build(),
            ModelInfoBuilder::new(MODEL_MINIMAX_M2_1)
                .context(204_800, 32_768)
                .capabilities(false, true, true, true)
                .thinking_required()
                .pricing(0.0, 0.0)
                .build(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regional_hosts_match_docs() {
        assert_eq!(
            BailianRegion::Beijing.host(),
            "cn-beijing.maas.aliyuncs.com"
        );
        assert_eq!(
            BailianRegion::Singapore.host(),
            "ap-southeast-1.maas.aliyuncs.com"
        );
        assert_eq!(
            BailianRegion::UsVirginia.host(),
            "us-east-1.maas.aliyuncs.com"
        );
        assert_eq!(
            BailianRegion::GermanyFrankfurt.host(),
            "eu-central-1.maas.aliyuncs.com"
        );
        assert_eq!(
            BailianRegion::JapanTokyo.host(),
            "ap-northeast-1.maas.aliyuncs.com"
        );
    }

    #[test]
    fn token_plan_domains_match_docs() {
        assert_eq!(
            BailianTokenPlanRegion::Domestic.base_url(),
            "https://coding.dashscope.aliyuncs.com/apps/anthropic"
        );
        assert_eq!(
            BailianTokenPlanRegion::International.base_url(),
            "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
        );
    }

    #[test]
    fn default_endpoint_is_beijing_region() {
        assert_eq!(
            BailianEndpoint::default(),
            BailianEndpoint::Main(BailianRegion::Beijing)
        );
    }

    #[test]
    fn preset_urls_expose_workspace_placeholder() {
        assert_eq!(
            BailianProvider::default_base_url(),
            "https://<WORKSPACE-ID>.cn-beijing.maas.aliyuncs.com/apps/anthropic"
        );
        let urls = BailianProvider::known_base_urls();
        assert!(urls.iter().any(|u| u.contains("maas.aliyuncs.com")));
        assert!(urls
            .iter()
            .any(|u| u.contains("coding.dashscope.aliyuncs.com")));
        assert!(urls
            .iter()
            .any(|u| u.contains("coding-intl.dashscope.aliyuncs.com")));
    }

    #[test]
    fn thinking_required_models_have_required_flag() {
        let required = [
            MODEL_KIMI_K2_7_CODE,
            MODEL_KIMI_K2_THINKING,
            MODEL_MINIMAX_M2_5,
            MODEL_MINIMAX_M2_1,
        ];
        for model in BailianProvider::preset_models() {
            if required.contains(&model.model_name.as_str()) {
                assert!(model.thinking_required, "{}", model.model_name);
                assert!(model.thinking_enabled, "{}", model.model_name);
                assert!(model.supports_thinking, "{}", model.model_name);
            }
        }
    }

    #[test]
    fn vision_models_have_vision_flag() {
        let vision = [
            MODEL_QWEN3_VL_PLUS,
            MODEL_QWEN3_VL_FLASH,
            MODEL_QWEN_VL_MAX,
            MODEL_QWEN_VL_PLUS,
            MODEL_KIMI_K3,
        ];
        for model in BailianProvider::preset_models() {
            if vision.contains(&model.model_name.as_str()) {
                assert!(model.vision_ability, "{}", model.model_name);
            }
        }
    }

    #[test]
    fn preset_models_have_nil_provider_id() {
        use uuid::Uuid;
        for model in BailianProvider::preset_models() {
            assert_eq!(model.provider_id, Uuid::nil(), "{}", model.model_name);
        }
    }
}