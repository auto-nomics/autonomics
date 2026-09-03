/**
 * HomepageStrategy —— AI Chat 首页（无关联论文）的 prompt 策略
 *
 * 启用 layer:
 * - persona（homepage persona，CORE_PERSONA_RULES + 通用助手语调）
 * - threadContext（用户在首页的追问历史）
 * - additionalHigh/additionalLow（AntDesign 注入 hook 仍可工作）
 *
 * 不启用：
 * - paperInfo（首页没有关联论文，不传论文元信息）
 *
 * @module ai-chat/strategies/HomepageStrategy
 */

import type { PromptStrategy } from './PromptStrategy';

export const HomepageStrategy: PromptStrategy = {
  agentType: 'homepage',
  enabledLayers: new Set([
    'persona',
    'threadContext',
    'additionalHigh',
    'additionalLow',
  ]),
  description: 'AI Chat 首页：通用助手 + 追问历史，不绑定论文',
};
