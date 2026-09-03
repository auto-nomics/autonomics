/**
 * PromptStrategy —— AgentType 到 System Prompt 构建策略的抽象
 *
 * Phase 5 引入：把"哪个 agentType 启用哪些 Layer"的决策从 buildSystemPromptBlocks
 * 内部的隐式 condition 提到 strategy 类，让数据流真正以 agentType 为轴心。
 *
 * 设计要点：
 * - **enabledLayers 是声明式的**：strategy 只声明它"想用"哪些 layer，
 *   builder 仍按数据存在性 + token 预算决定是否真正注入（保留 graceful degradation）
 * - **不承载渲染逻辑**：layer 渲染继续在 systemPromptBuilder 内（避免 4 个 strategy 重复代码）
 * - **registry-driven**：getStrategy(agentType) 是唯一访问点，方便扩展第 5 种 agentType
 *
 * 与原 builder 的对应关系：
 * | Layer | 当前条件                  | PaperReader | Homepage | Screening | Kb |
 * |-------|---------------------------|-------------|----------|-----------|----|
 * | 1     | persona（always）         | ✓           | ✓        | ✓         | ✓  |
 * | 1(元) | paperInfo ≠ null          | ✓           | -        | ✓         | -  |
 * | 1.5   | threadContextInjection    | ✓           | ✓        | ✓         | ✓  |
 * | 2.5   | webSearchEnabled          | ✓           | ✓        | ✓         | ✓  |
 * | 2.7   | additionalSystemContext   | ✓           | ✓        | ✓         | ✓  |
 * | 3     | guideSections.length > 0  | ✓           | -        | -         | -  |
 * | 5     | additionalSystemContext   | ✓           | ✓        | ✓         | ✓  |
 *
 * 表格里的 ✓/- 是 strategy 的声明；builder 内仍是 if (data) 渲染。
 *
 * @module ai-chat/strategies/PromptStrategy
 */

import type { AgentType } from '../agentTypes';

/** 所有可能的 Layer 标识（与 systemPromptBuilder 内的渲染分支一一对应） */
export type PromptLayer =
  | 'persona'              // Layer 1: base persona（按 agentType 取 DEFAULT_PERSONAS）
  | 'paperInfo'            // Layer 1 元信息：标题/作者/摘要（需要 paperInfo ≠ null）
  | 'threadContext'        // Layer 1.5: 用户之前追问及回复摘要（主对话）/ 锚文本+父回复（ThreadStrategy）
  | 'webSearch'            // Layer 2.5: Tavily 搜索结果（动态）
  | 'additionalHigh'       // Layer 2.7: 高优先级 additionalSystemContext
  | 'guideSections'        // Layer 3: 段落导览（需要 guideSections.length > 0）
  | 'additionalLow';       // Layer 5: 低优先级 additionalSystemContext

/**
 * 构建上下文 —— strategy 与 builder 共享的数据包
 *
 * 与原 BuildSystemPromptParams 字段一致，但全部 optional（由 strategy 决定哪些字段真正被读）。
 * 把 setter (setMessages/setWebSearching/...) 也包进来，避免 builder 接收 18 个参数。
 */
export interface PromptBuildContext {
  paperInfo?: { title?: string; authors?: string | string[]; abstract?: string } | null;
  paperId?: string | number | null;
  paper?: { parse_status?: string } | null;
  guideSections?: Array<{ section_title: string; paragraphs: Array<{ importance?: number; summary: string }> }>;
  messages?: Array<Record<string, any>>;
  additionalSystemContext?: Array<{ text: string; priority: string }> | null;
  threadContextInjection?: boolean;
  customSystemPrompt?: string;
  webSearchEnabled?: boolean;
  userContent?: string | Array<Record<string, any>>;
  newMessages?: Array<Record<string, any>>;
  tokenBudget?: number;
}

/** Strategy 声明的渲染偏好（不包含数据本身） */
export interface PromptStrategy {
  /** 该 strategy 对应的 agentType（用于 persona 取值 + 反查） */
  readonly agentType: AgentType;
  /** 该 strategy 声明启用的 layer 集合（builder 仍按数据存在性最终决定） */
  readonly enabledLayers: ReadonlySet<PromptLayer>;
  /** 一句话描述该 strategy 的设计意图，便于调试与文档 */
  readonly description: string;
}
