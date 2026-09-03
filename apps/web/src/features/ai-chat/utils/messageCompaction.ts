/**
 * 对话历史压缩工具模块
 *
 * 从 useChatSender.js 中提取的纯工具函数，用于压缩长对话历史。
 * 无 React 依赖，可作为独立模块导入使用。
 *
 * @module ai-chat/utils/messageCompaction
 */

import { summarizeConversation } from '../../../services/chatApi'; // 导入对话摘要 API
import { estimateTokens } from '../../../utils/markdownUtils'; // 导入 token 估算工具
import { stripAttachmentsForPersistence } from '../utils/chatMessageUtils'; // 导入消息清理工具

// ========== 渐进式历史压缩配置 ==========

/**
 * 压缩 Promise 引用（每个论文独立）
 *
 * 用于防止并发压缩：如果同一论文的上一次压缩仍在进行中，新的压缩会等待其完成。
 * 使用 Map 实现每个论文独立的锁，避免不同论文之间的并发阻塞。
 *
 * @type {Map<number, Promise>}
 */
const compactPromises = new Map();

/**
 * 获取压缩 Promise 引用（用于测试和外部访问）
 *
 * @param {number} paperId - 论文 ID
 * @returns {Promise|null} 该论文的压缩 Promise
 */
export function getCompactPromise(paperId: any) {
  return compactPromises.get(paperId);
}

/**
 * 计算对话历史压缩的 token 阈值
 *
 * 压缩阈值 = max(tokenBudget * 0.6, 10000)
 * - 最低 10000 tokens：确保即使预算很小也能进行压缩
 * - 预算的 60%：在 token 预算的 60% 处触发压缩，留出 40% 余量
 *
 * @param {number} tokenBudget - 当前模型的 token 预算
 * @returns {number} 压缩阈值（tokens）
 */
export function getCompactThreshold(tokenBudget: any) {
  return Math.max(tokenBudget * 0.6, 10000);
}

/**
 * 渐进式压缩对话历史
 *
 * 将长对话历史压缩为结构化摘要，降低 token 消耗。
 *
 * @param {Array} cleanMessages - 清理后的消息列表（不含错误消息）
 * @param {number} tokenBudget - 当前模型的 token 预算
 * @param {string} paperTitle - 论文标题（用于摘要上下文）
 * @param {number} paperId - 论文 ID（用于调用后端 API）
 * @param {boolean} isAgentMode - 是否为 Agent 模式（Agent 模式下跳过前端压缩，由后端处理）
 * @returns {Promise<{messages: Array, fallbackReason: string|null, errorMessage: string|null}>} 压缩结果
 */
export async function compactHistory(cleanMessages: any, tokenBudget: any, paperTitle: any, paperId: any, isAgentMode: any = false) {
  // Agent 模式下跳过前端压缩（后端 memory.rs 会处理）
  if (isAgentMode) {
    return { messages: cleanMessages, fallbackReason: null, errorMessage: null };
  }

  // 第 1 步：等待当前论文的并发锁
  const existingPromise = compactPromises.get(paperId);
  if (existingPromise) {
    await existingPromise;
  }

  // 第 2 步：计算总 token
  let totalTokens = 0;
  for (const msg of cleanMessages) {
    const msgForToken = { role: msg.role, content: msg.content };
    const serialized = typeof msg.content === 'string'
      ? msg.content
      : JSON.stringify(msgForToken);
    totalTokens += estimateTokens(serialized);
  }

  // 第 3 步：检查是否达到压缩阈值
  const threshold = getCompactThreshold(tokenBudget);
  if (totalTokens < threshold) {
    return { messages: cleanMessages, fallbackReason: null, errorMessage: null };
  }

  // 第 4 步：检查是否已有摘要消息
  const existingSummaryIndex = cleanMessages.findIndex(
    (m: any) => m.role === 'user' && typeof m.content === 'string' && m.content.startsWith('[上下文摘要]')
  );
  if (existingSummaryIndex !== -1) {
    const keepCount = 20;
    const summaryAndRecent = [
      ...cleanMessages.slice(existingSummaryIndex, existingSummaryIndex + 1),
      ...cleanMessages.slice(-keepCount),
    ];
    const uniqueMessages = Array.from(
      new Map(summaryAndRecent.map((m: any) => {
        const dedupKey = `${m.role}:${typeof m.content === 'string' ? m.content : JSON.stringify(m.content)}`;
        return [dedupKey, m];
      })).values()
    );
    return { messages: uniqueMessages, fallbackReason: null, errorMessage: null };
  }

  // 第 5 步：分割旧消息和最近消息
  const recentCount = 20;
  const oldMessages = cleanMessages.slice(0, -recentCount);
  const recentMessages = cleanMessages.slice(-recentCount);

  if (oldMessages.length === 0) {
    return { messages: cleanMessages, fallbackReason: null, errorMessage: null };
  }

  // 第 6 步：调用后端 API 生成摘要
  try {
    const doSummarize = async () => {
      const strippedOldMessages = stripThreadsForApi(oldMessages);
      const result = await summarizeConversation(paperId, strippedOldMessages, paperTitle);

      // 第 7 步：构建摘要消息
      const summaryMessage = {
        role: 'user',
        content: `[上下文摘要]\n\n${result.summary}`,
      };

      const compressedMessages = [summaryMessage, ...recentMessages];
      return compressedMessages;
    };

    const summarizePromise = doSummarize();
    compactPromises.set(paperId, summarizePromise);
    const compressedMessages = await summarizePromise;
    return { messages: compressedMessages, fallbackReason: null, errorMessage: null };

  } catch (err: any) {
    const errMsg = err?.message || String(err);
    console.error('[compactHistory] 摘要 API 调用失败:', errMsg);
    throw new Error(`对话压缩失败: ${errMsg}`);
  } finally {
    compactPromises.delete(paperId);
  }
}

/**
 * 从消息数组中移除 threads 字段（用于发送给 AI API）
 *
 * @param {Array} messages - 原始消息数组（可能包含 threads 字段）
 * @returns {Array} 清理后的消息数组（不含 threads 字段）
 */
export function stripThreadsForApi(messages: any) {
  return messages.map((m: any) => {
    const { role, content, id } = m;
    return { role, content, id };
  });
}
