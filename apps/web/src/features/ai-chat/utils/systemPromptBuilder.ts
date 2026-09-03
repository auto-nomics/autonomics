/**
 * 分层系统提示词构建工具模块
 *
 * 从 useChatSender.js 中提取的 buildSystemPrompt 函数。
 * 按 PLAN.md 定义的分层上下文注入策略构建 system prompt：
 * - Layer 1: 论文基础信息（固定注入）
 * - Layer 1.5: 线程上下文注入（Phase 6，按需注入）
 * - Layer 2.5: 网络搜索结果（按需注入）
 * - Layer 2.7: 额外系统上下文高优先级（按需注入）
 * - Layer 3: 段落导览上下文（按需注入）
 * - Layer 5: 额外系统上下文低优先级（按需注入）
 *
 * Prompt Caching (Phase 2-P3):
 * - Tier 1 (stable, cache): Layer 1 (core prompt)
 * - Tier 2 (semi-stable, cache): Layer 1.5 (thread context), Layer 2.7 (high priority), Layer 3 (paragraph guide)
 * - Tier 3 (dynamic, no cache): Layer 2.5 (search results), Layer 5 (low priority)
 *
 * @module ai-chat/utils/systemPromptBuilder
 */

import type { WebSearchResponse, SearchResult } from '@/types';
import { webSearch } from '../../../services/searchApi';
import { estimateTokens } from '../../../utils/markdownUtils';
import { extractTextFromContent } from '../utils/chatMessageUtils';
import type { AgentType } from '../agentTypes';
import { DEFAULT_PERSONAS } from '../agentTypes';
import { getStrategy } from '../strategies/strategyRegistry';
import {
  CacheStrategy,
  createBlock,
  flattenBlocks,
  type CacheControlBlock,
} from './promptUtils';

/**
 * 构建分层系统提示词（带缓存控制）
 *
 * 返回结构化的 CacheControlBlock[]，用于 Anthropic prompt caching。
 * 每个块根据其稳定性 tier 决定是否添加 cache_control 标记。
 *
 * @param {object} params - 参数对象
 * @param {object} params.paperInfo - 论文基础信息（标题、作者、摘要）
 * @param {string|number} params.paperId - 论文 ID（用于 RAG 检索）
 * @param {object} params.paper - 论文完整信息（包含 parse_status）
 * @param {Array} params.guideSections - 段落导览数据
 * @param {Array} params.messages - 当前消息列表（用于线程上下文）
 * @param {Array|null} params.additionalSystemContext - 额外系统上下文注入
 * @param {boolean} params.threadContextInjection - 线程上下文注入开关
 * @param {Function} params.setMessages - 设置消息列表的函数
 * @param {Function} params.setWebSearching - 设置搜索状态的函数
 * @param {Function} params.setLastWebSearchSources - 设置搜索来源的函数
 * @param {boolean} params.webSearchEnabled - 是否开启网络搜索
 * @param {any} params.userContent - 用户消息内容
 * @param {Array} params.newMessages - 新消息列表
 * @param {number} params.tokenBudget - token 预算
 * @returns {Promise<{systemPrompt: CacheControlBlock[], error: string|null}>} 系统提示词块和可能的错误信息
 */
export async function buildSystemPromptBlocks({
  agentType,
  paperInfo,
  paperId,
  paper,
  guideSections,
  messages,
  additionalSystemContext = null,
  threadContextInjection = false,
  setMessages,
  setWebSearching,
  setLastWebSearchSources,
  webSearchEnabled = false,
  userContent,
  newMessages,
  tokenBudget,
  customSystemPrompt = '',
}: {
  agentType: AgentType;
  paperInfo: { title?: string; authors?: string | string[]; abstract?: string } | null;
  paperId: string | number | null;
  paper: { parse_status?: string } | null;
  guideSections: Array<{ section_title: string; paragraphs: Array<{ importance?: number; summary: string }> }>;
  messages: Array<Record<string, any>>;
  additionalSystemContext: Array<{ text: string; priority: string }> | null;
  threadContextInjection: boolean;
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  setWebSearching: React.Dispatch<React.SetStateAction<boolean>>;
  setLastWebSearchSources: React.Dispatch<React.SetStateAction<Array<{ title: string; url: string }>>>;
  webSearchEnabled: boolean;
  userContent: string | Array<Record<string, any>>;
  newMessages: Array<Record<string, any>>;
  tokenBudget: number;
  customSystemPrompt?: string;
}): Promise<{ systemPrompt: CacheControlBlock[] | string; error: string | null }> {
  // 按缓存层级分组：Tier 1 -> Tier 2 -> Tier 3
  const tier1Blocks: CacheControlBlock[] = [];  // 稳定内容，强缓存
  const tier2Blocks: CacheControlBlock[] = [];  // 半稳定内容，缓存
  const tier3Blocks: CacheControlBlock[] = [];  // 动态内容，不缓存

  // Phase 5: 通过 strategy 显式声明启用哪些 layer。
  // strategy.enabledLayers 是声明式的"想用"，builder 仍按数据存在性最终决定渲染与否。
  const strategy = getStrategy(agentType);

  // ===== Tier 1: Layer 1 - 论文基础信息（固定注入，最稳定）=====
  // v2 (2026-06-28): persona 由 agentType 字典决定（homepage/paperReader/screening），
  // 用户可通过 customSystemPrompt 覆盖当前 agentType 的默认 persona。
  const basePersona = customSystemPrompt?.trim() || DEFAULT_PERSONAS[agentType];

  if (strategy.enabledLayers.has('paperInfo') && paperInfo) {
    const layer1Parts = [
      customSystemPrompt?.trim()
        ? basePersona
        : `${basePersona}\n\n以下是当前论文的元信息，请基于此回答：`,
      `## 论文信息`,
      `**标题**: ${paperInfo.title || '未知'}`,
      paperInfo.authors ? `**作者**: ${Array.isArray(paperInfo.authors) ? paperInfo.authors.join(', ') : paperInfo.authors}` : '',
      paperInfo.abstract ? `\n## 论文摘要\n${paperInfo.abstract}` : '',
    ].filter(Boolean).join('\n');
    tier1Blocks.push(createBlock(layer1Parts, CacheStrategy.STABLE));
  } else {
    tier1Blocks.push(createBlock(basePersona, CacheStrategy.STABLE));
  }

  // ===== Tier 2: Layer 1.5 - 线程上下文注入（半稳定，缓存）=====
  if (strategy.enabledLayers.has('threadContext') && threadContextInjection) {
    const threadSummaries = [];
    let summaryCount = 0;
    const MAX_THREAD_SUMMARIES = 3;
    const MAX_SUMMARY_LENGTH = 200;

    for (let i = messages.length - 1; i >= 0 && summaryCount < MAX_THREAD_SUMMARIES; i--) {
      const msg = messages[i];
      const threads = msg?.threads || [];

      if (threads.length === 0) continue;

      for (const thread of threads) {
        const threadMessages = thread?.messages || [];
        if (threadMessages.length === 0) continue;

        let lastAiReply = null;
        for (let j = threadMessages.length - 1; j >= 0; j--) {
          if (threadMessages[j].role === 'assistant') {
            lastAiReply = threadMessages[j];
            break;
          }
        }

        if (lastAiReply) {
          const replyText = extractTextFromContent(lastAiReply.content);
          const truncatedReply = replyText.length > MAX_SUMMARY_LENGTH
            ? replyText.slice(0, MAX_SUMMARY_LENGTH) + '...'
            : replyText;

          const anchorText = thread.anchorText || '选定文本';

          threadSummaries.push({
            anchorText,
            reply: truncatedReply,
          });

          summaryCount++;
          if (summaryCount >= MAX_THREAD_SUMMARIES) break;
        }
      }
    }

    if (threadSummaries.length > 0) {
      threadSummaries.reverse();

      const threadContextText = [
        '## 用户之前的追问及回复',
        '以下是用户之前针对你回答中特定段落的追问及你的回复摘要：',
        ...threadSummaries.map((s, idx) => `${idx + 1}. 关于「${s.anchorText}」：${s.reply}`),
      ].join('\n');

      tier2Blocks.push(createBlock(threadContextText, CacheStrategy.SEMI_STABLE));
    }
  }

  // ===== Tier 3: Layer 2.5 - 网络搜索结果（动态，不缓存）=====
  if (strategy.enabledLayers.has('webSearch') && webSearchEnabled) {
    setWebSearching(true);

    try {
      const searchQuery = extractTextFromContent(userContent);

      setMessages(prev => [
        ...prev,
        { role: 'assistant', content: '__WEB_SEARCHING__' },
      ]);

      const searchResult = await webSearch(searchQuery) as WebSearchResponse;

      if (searchResult.formatted_prompt) {
        tier3Blocks.push(createBlock(searchResult.formatted_prompt, CacheStrategy.DYNAMIC));

        setLastWebSearchSources(
          (searchResult.results || []).map((r: SearchResult) => ({
            title: r.title || '无标题',
            url: r.url || '',
          }))
        );
      } else if (searchResult.error) {
        console.warn('网络搜索失败:', searchResult.error);

        if (searchResult.error.includes('未配置') || searchResult.error.includes('API Key')) {
          return { systemPrompt: '', error: 'API_KEY_MISSING' };
        }
      }
    } catch (err: any) {
      console.warn('网络搜索请求失败:', err.message);

      if (err.message && err.message.includes('未配置')) {
        return { systemPrompt: '', error: 'API_KEY_MISSING' };
      }
    } finally {
      // Always remove the placeholder and reset webSearching state
      setMessages(prev => prev.filter(m => m.content !== '__WEB_SEARCHING__'));
      setWebSearching(false);
    }
  }

  // ===== 计算剩余预算 =====
  const estimateBlocksTokens = (blocks: Array<{ text: string }>): number => {
    return blocks.reduce((sum, b) => sum + estimateTokens(b.text), 0);
  };
  const tier1Tokens = estimateBlocksTokens(tier1Blocks);
  const tier2Tokens = estimateBlocksTokens(tier2Blocks);
  const tier3Tokens = estimateBlocksTokens(tier3Blocks);
  let remainingBudget = tokenBudget - tier1Tokens - tier2Tokens - tier3Tokens - 2000;

  // ===== Tier 2: Layer 2.7 - 额外系统上下文（高优先级，半稳定，缓存）=====
  let highPriorityInsertIndex = tier2Blocks.length;
  if (strategy.enabledLayers.has('additionalHigh') && additionalSystemContext && additionalSystemContext.length > 0) {
    const highPriority = additionalSystemContext.filter(ctx => ctx.priority === 'high');
    for (const ctx of highPriority) {
      const tokens = estimateTokens(ctx.text);
      if (remainingBudget > tokens) {
        tier2Blocks.splice(highPriorityInsertIndex, 0, createBlock(ctx.text, CacheStrategy.SEMI_STABLE));
        highPriorityInsertIndex++;
        remainingBudget -= tokens;
      }
    }
  }

  // ===== Tier 2: Layer 3 - 段落导览上下文（半稳定，缓存）=====
  if (strategy.enabledLayers.has('guideSections') && guideSections.length > 0 && remainingBudget > 0) {
    const guideLines = [];
    for (const section of guideSections) {
      guideLines.push(`### ${section.section_title}`);
      for (const para of section.paragraphs) {
        const imp = para.importance || 3;
        const stars = '★'.repeat(imp);
        guideLines.push(`- [${stars}] ${para.summary}`);
      }
    }
    const guideText = guideLines.join('\n');
    const guideTokens = estimateTokens(guideText);

    if (guideTokens <= remainingBudget) {
      tier2Blocks.push(createBlock(`## 论文段落导览\n${guideText}`, CacheStrategy.SEMI_STABLE));
    } else {
      const importantLines = [];
      for (const section of guideSections) {
        const importantParas = section.paragraphs.filter(p => (p.importance || 0) >= 4);
        if (importantParas.length > 0) {
          importantLines.push(`### ${section.section_title}`);
          for (const para of importantParas) {
            importantLines.push(`- [★★★★] ${para.summary}`);
          }
        }
      }
      if (importantLines.length > 0) {
        tier2Blocks.push(createBlock(`## 论文段落导览（仅重要段落）\n${importantLines.join('\n')}`, CacheStrategy.SEMI_STABLE));
      }
    }

    // 重新计算剩余预算（包含新增的 Tier 2 内容）
    const updatedTier2Tokens = estimateBlocksTokens(tier2Blocks);
    remainingBudget = tokenBudget - tier1Tokens - updatedTier2Tokens - tier3Tokens - 2000;
  }

  // Tier 3 之前的 RAG 语义检索层已移除（向量嵌入 RAG 已下线）。

  // ===== Tier 3: Layer 5 - 额外系统上下文（低优先级，动态，不缓存）=====
  if (strategy.enabledLayers.has('additionalLow') && additionalSystemContext && additionalSystemContext.length > 0) {
    const lowPriority = additionalSystemContext.filter(ctx => ctx.priority !== 'high');
    for (const ctx of lowPriority) {
      const tokens = estimateTokens(ctx.text);
      if (remainingBudget > tokens) {
        tier3Blocks.push(createBlock(ctx.text, CacheStrategy.DYNAMIC));
        remainingBudget -= tokens;
      }
    }
  }

  // 合并所有层级：Tier 1 -> Tier 2 -> Tier 3
  // Anthropic 要求 cache_breakpoint 后的内容必须连续匹配才能命中缓存
  // 因此按稳定性递减顺序排列，最大化缓存命中率
  const systemPromptBlocks = [...tier1Blocks, ...tier2Blocks, ...tier3Blocks];

  return { systemPrompt: systemPromptBlocks, error: null };
}

/**
 * 构建分层系统提示词参数类型
 *
 * 主对话路径（buildSystemPromptBlocks）和向后兼容路径（buildSystemPrompt）共享同一组入参。
 */
export interface BuildSystemPromptParams {
  agentType: AgentType;
  paperInfo: { title?: string; authors?: string | string[]; abstract?: string } | null;
  paperId: string | number | null;
  paper: { parse_status?: string } | null;
  guideSections: Array<{ section_title: string; paragraphs: Array<{ importance?: number; summary: string }> }>;
  messages: Array<Record<string, any>>;
  additionalSystemContext: Array<{ text: string; priority: string }> | null;
  threadContextInjection: boolean;
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  setWebSearching: React.Dispatch<React.SetStateAction<boolean>>;
  setLastWebSearchSources: React.Dispatch<React.SetStateAction<Array<{ title: string; url: string }>>>;
  webSearchEnabled: boolean;
  userContent: string | Array<Record<string, any>>;
  newMessages: Array<Record<string, any>>;
  tokenBudget: number;
  customSystemPrompt?: string;
}

/**
 * 构建分层系统提示词（向后兼容版本）
 *
 * 返回扁平字符串格式，用于非 Anthropic 提供商或向后兼容。
 * 内部调用 buildSystemPromptBlocks() 并通过 flattenBlocks() 拼接为字符串。
 *
 * @param {BuildSystemPromptParams} params - 参数对象（与 buildSystemPromptBlocks 相同）
 * @returns {Promise<{systemPrompt: string, error: string|null}>} 系统提示词字符串和可能的错误信息
 */
export async function buildSystemPrompt(params: BuildSystemPromptParams): Promise<{ systemPrompt: string; error: string | null }> {
  const result = await buildSystemPromptBlocks(params);

  return {
    systemPrompt: flattenBlocks(result.systemPrompt),
    error: result.error,
  };
}
