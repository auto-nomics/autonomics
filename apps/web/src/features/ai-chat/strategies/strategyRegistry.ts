/**
 * strategyRegistry —— AgentType 到 PromptStrategy 的唯一映射
 *
 * 所有 agentType 必须在此注册。getStrategy 是 builder 的唯一访问点。
 *
 * 添加新 agentType 的步骤：
 * 1. 在 agentTypes.ts 加 AgentType 字面量 + DEFAULT_PERSONAS
 * 2. 在 strategies/ 新建 XxxStrategy.ts
 * 3. 在本文件 REGISTRY 加映射
 *
 * @module ai-chat/strategies/strategyRegistry
 */

import type { AgentType } from '../agentTypes';
import type { PromptStrategy } from './PromptStrategy';
import { HomepageStrategy } from './HomepageStrategy';
import { PaperReaderStrategy } from './PaperReaderStrategy';
import { ScreeningStrategy } from './ScreeningStrategy';

const REGISTRY: Record<AgentType, PromptStrategy> = {
  homepage: HomepageStrategy,
  paperReader: PaperReaderStrategy,
  screening: ScreeningStrategy,
};

/**
 * 获取指定 agentType 的 PromptStrategy
 *
 * 未注册的 agentType 会抛错（fail-fast）—— 防止 silent fallback 到默认 strategy
 * 导致 prompt 内容意外变化。
 */
export function getStrategy(agentType: AgentType): PromptStrategy {
  const strategy = REGISTRY[agentType];
  if (!strategy) {
    throw new Error(`[strategyRegistry] 未注册的 agentType: ${agentType}`);
  }
  return strategy;
}

/** 列出所有已注册的 strategy（调试用） */
export function listStrategies(): PromptStrategy[] {
  return Object.values(REGISTRY);
}
