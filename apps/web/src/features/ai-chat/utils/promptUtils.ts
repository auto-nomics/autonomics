/**
 * 系统提示词构建共享工具
 *
 * 从 systemPromptBuilder.ts / threadUtils.ts / useChatSender.ts 抽出的公共逻辑：
 * - CacheControlBlock 类型 + CacheStrategy 枚举（Anthropic prompt caching 用）
 * - createBlock: 按 tier 创建带 cache_control 的文本块
 * - flattenBlocks: 把 CacheControlBlock[] 拼成单字符串（向后兼容非 Anthropic 提供商）
 * - formatPaperContext: 把 paperInfo 渲染成【论文上下文】文本块
 *
 * @module ai-chat/utils/promptUtils
 */

/** 论文基础信息（标题/作者/摘要/年份），由多个 builder 共享 */
export interface PaperContextInfo {
  title?: string;
  authors?: string | string[];
  abstract?: string;
  year?: number | string;
}

/**
 * Anthropic prompt caching 控制块
 *
 * - type: 固定 'text'
 * - text: 文本内容
 * - cache_control: 仅 Tier 1 (STABLE) / Tier 2 (SEMI_STABLE) 块带此字段
 */
export interface CacheControlBlock {
  type: 'text';
  text: string;
  cache_control?: { type: 'ephemeral' };
}

/**
 * 缓存策略 tier
 *
 * - STABLE: 稳定内容（Layer 1 persona + 论文元信息），强缓存
 * - SEMI_STABLE: 半稳定内容（Layer 1.5 线程摘要、Layer 2.7 高优先级、Layer 3 段落导览），缓存
 * - DYNAMIC: 动态内容（Layer 2.5 网络搜索结果、Layer 5 低优先级），不缓存
 */
export const CacheStrategy = {
  STABLE: 'stable',
  SEMI_STABLE: 'semi_stable',
  DYNAMIC: 'dynamic',
} as const;

export type CacheStrategyValue = typeof CacheStrategy[keyof typeof CacheStrategy];

/**
 * 创建带缓存控制的文本块
 *
 * Tier 1 (STABLE) / Tier 2 (SEMI_STABLE) 块加 cache_control: { type: 'ephemeral' }；
 * Tier 3 (DYNAMIC) 不加。
 *
 * Anthropic 要求 cache breakpoint 之后的内容必须连续匹配才能命中缓存，
 * 因此调用方需按 tier 递减顺序拼接（tier1Blocks → tier2Blocks → tier3Blocks）。
 */
export function createBlock(text: string, cacheStrategy: CacheStrategyValue | string): CacheControlBlock {
  const block: CacheControlBlock = { type: 'text', text };
  if (cacheStrategy === CacheStrategy.STABLE || cacheStrategy === CacheStrategy.SEMI_STABLE) {
    block.cache_control = { type: 'ephemeral' };
  }
  return block;
}

/**
 * 把 CacheControlBlock[] 拼成单字符串
 *
 * 用于非 Anthropic 提供商（OpenAI 兼容）或向后兼容路径。
 * 块之间用 "\n\n---\n\n" 分隔。
 *
 * 入参可能是 string（线程模式 buildThreadSystemPrompt 已是字符串）或 CacheControlBlock[]。
 */
export function flattenBlocks(blocks: CacheControlBlock[] | string): string {
  if (typeof blocks === 'string') return blocks;
  return blocks.map(b => b.text).join('\n\n---\n\n');
}

/**
 * 把 paperInfo 渲染成【论文上下文】文本块
 *
 * 用于 thread / sub-thread 系统提示词 prepend（主对话路径在 buildSystemPromptBlocks 内联处理）。
 * 输出格式：
 *
 *   【论文上下文】
 *   标题：...
 *   作者：...      ← 仅当 authors 存在
 *   摘要：...      ← 仅当 abstract 存在
 *   年份：...      ← 仅当 year 存在
 */
export function formatPaperContext(paperInfo: PaperContextInfo | null): string | null {
  if (!paperInfo) return null;

  const lines: string[] = [`【论文上下文】`, `标题：${paperInfo.title || ''}`];

  if (paperInfo.authors) {
    lines.push(`作者：${paperInfo.authors}`);
  }
  if (paperInfo.abstract) {
    lines.push(`摘要：${paperInfo.abstract}`);
  }
  if (paperInfo.year) {
    lines.push(`年份：${paperInfo.year}`);
  }

  return lines.join('\n');
}
