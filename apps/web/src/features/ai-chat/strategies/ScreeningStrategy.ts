/**
 * ScreeningStrategy —— 筛选模式（多论文对比）的 prompt 策略
 *
 * 启用：
 * - persona（screening persona，对比/筛选语调）
 * - paperInfo（用于多篇对比，但每条消息只附 1 篇元信息）
 * - threadContext（用户在筛选过程中的追问）
 * - additionalHigh/additionalLow（筛选用对比表等高层注入）
 *
 * 不启用：
 *
 * @module ai-chat/strategies/ScreeningStrategy
 */

import type { PromptStrategy } from './PromptStrategy';

export const ScreeningStrategy: PromptStrategy = {
  agentType: 'screening',
  enabledLayers: new Set([
    'persona',
    'paperInfo',
    'threadContext',
    'additionalHigh',
    'additionalLow',
  ]),
  description: '筛选模式：论文元信息 + 对比 + 追问，不看段落导览',
};
