/**
 * PaperReaderStrategy —— 论文阅读面板的 prompt 策略
 *
 * 启用所有 layer —— 这是最"重"的策略：
 * - persona（paperReader persona，4 件套严格约束）
 * - paperInfo（论文标题/作者/摘要）
 * - threadContext（用户针对段落的追问历史）
 * - webSearch（论文相关问题触发外部检索）
 * - additionalHigh/additionalLow（图片系统上下文等高层注入）
 * - guideSections（Layer 3 段落导览，仅 paperReader 启用）
 *
 * @module ai-chat/strategies/PaperReaderStrategy
 */

import type { PromptStrategy } from './PromptStrategy';

export const PaperReaderStrategy: PromptStrategy = {
  agentType: 'paperReader',
  enabledLayers: new Set([
    'persona',
    'paperInfo',
    'threadContext',
    'webSearch',
    'additionalHigh',
    'guideSections',
    'additionalLow',
  ]),
  description: '论文阅读：完整论文上下文 + 段落导览 + 追问 + web search',
};
