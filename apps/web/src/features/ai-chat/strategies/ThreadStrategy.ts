/**
 * ThreadStrategy —— 追问线程（inline thread）的 prompt 策略
 *
 * 与 HomepageStrategy/PaperReaderStrategy 不同，ThreadStrategy 不在
 * strategyRegistry 的 REGISTRY 里，因为：
 * - 它不对应一个独立的 AgentType（'homepage'|'paperReader'|'screening'）
 * - 它是任何 agentType 的"子模式"（在线程激活时叠加在父 strategy 之上）
 * - 它的 persona 是专门的「追问模式」提示，不走 DEFAULT_PERSONAS[agentType]
 *
 * 当前状态（Phase 6 准备阶段）：
 * - 本 strategy 作为架构占位 + 文档存在
 * - 实际线程 prompt 仍由 buildThreadSystemPrompt 独立构建（保留 byte-level 输出）
 * - 未来路线：把 buildThreadSystemPrompt 内的渲染逻辑迁到 systemPromptBuilder
 *   的 threadContext layer，本 strategy 描述那条路径的 layer 偏好
 *
 * 与其他 strategy 的差异（设计意图）：
 * - 启用 threadContext layer（其他 strategy 都没这个）
 * - 不启用 paperInfo / guideSections（线程聚焦单段原文，不重渲染论文元信息）
 * - webSearch 默认开启（线程内允许触发外部检索 —— plan 的 Phase 6 可选目标）
 *
 * @module ai-chat/strategies/ThreadStrategy
 */

import type { PromptStrategy } from './PromptStrategy';

export const ThreadStrategy: PromptStrategy = {
  // 复用 paperReader 作为父 agentType（最常见场景）；
  // 实际线程路径不通过 strategyRegistry 解析，agentType 字段仅作元信息
  agentType: 'paperReader',
  enabledLayers: new Set([
    'persona',
    'threadContext',
    'webSearch',
    'additionalHigh',
    'additionalLow',
  ]),
  description: '追问线程：聚焦原文段落 + 父回复上下文 + 简洁直接风格',
};
