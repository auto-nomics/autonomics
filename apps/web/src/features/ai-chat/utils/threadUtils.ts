/**
 * 线程（Thread）工具函数
 *
 * 本模块提供内联追问线程的数据结构和核心操作函数。
 * 线程允许用户针对 AI 回答中的特定段落进行追问，追问内容显示在该段落下方。
 *
 * 核心设计决策：
 * - 使用 cyrb53 哈希算法计算文本指纹，用于 DOM 元素锚定
 * - 线程数据内联存储在消息对象中，避免独立状态管理
 * - 所有更新操作使用不可变模式，确保 React.memo 正常工作
 * - 支持线程失效标记（orphaned），当原始消息重新生成后标记线程为失效
 *
 * @module ai-chat/utils/threadUtils
 */

import { formatPaperContext, type PaperContextInfo } from './promptUtils';
import { estimateTokens } from '../../../utils/markdownUtils';
import type { ModelConfig } from './chatUtils';

// ============================================================
// 常量定义
// ============================================================

/**
 * 每个线程允许的最大消息数量
 *
 * 为什么限制？
 * - 线程设计用于简短追问，不是完整对话
 * - 防止单个线程膨胀导致 UI 卡顿
 * - 达到上限后显示提示并禁用输入
 */
export const MAX_THREAD_MESSAGES = 30;

/**
 * 每条消息允许的最大线程数量
 *
 * 为什么限制？
 * - 防止单条消息附件过多线程导致渲染性能下降
 * - 接近上限时前端提醒用户清理旧线程
 */
export const MAX_THREADS_PER_MESSAGE = 10;

/**
 * 每个 assistant 线程消息允许的最大子线程数量
 *
 * 为什么限制？
 * - 子线程嵌套在追问面板内，空间更有限
 * - 防止单个 assistant 消息上子线程过多导致渲染性能下降
 * - 接近上限时前端提醒用户清理旧子线程
 */
export const MAX_SUBTHREADS_PER_THREAD_MESSAGE = 5;

/**
 * 每个子线程允许的最大消息数量（与父线程一致）
 *
 * 为什么与父线程一致？
 * - 子线程本质是二级追问，对话模式与父线程相同
 * - 保持一致的上限便于用户理解和前端逻辑复用
 */
export const MAX_SUBTHREAD_MESSAGES = 30;

/**
 * 锚定文本（anchorText）的最大 token 数量
 *
 * 为什么限制？
 * - anchorText 作为用户选中的原文，用于线程上下文
 * - 过长的 anchorText 会占用大量 token 预算
 * - 超出限制时截断并追加 [已截断] 提示
 */
export const ANCHOR_TEXT_MAX_TOKENS = 1000;

/**
 * 线程消息的 token 预算（MVP 阶段使用固定值）
 *
 * 预算分配：
 * - Layer 1（论文元数据）：约 500 tokens
 * - anchorText：最多 1000 tokens
 * - 线程历史：最近 10 条消息
 * - AI 回答上下文：剩余空间
 *
 * 作为 computeDynamicThreadBudget 返回 null 时的 fallback 使用。
 * 主路径走动态计算（占总预算 30%，下限 2000，上限 8000）。
 */
export const THREAD_TOKEN_BUDGET = 3000;

/**
 * 线程 token 预算占主对话预算的比例
 *
 * 为什么是 30%？
 * - 主对话需要保留足够预算用于系统提示词、上下文注入
 * - 线程追问通常较短，30% 足够应对大多数场景
 * - 保留 70% 给主对话，确保主对话质量
 */
const THREAD_BUDGET_RATIO = 0.3;

/**
 * 线程 token 预算下限
 *
 * 为什么设置下限？
 * - 即使主对话预算很小，线程也需要最小可用空间
 * - 2000 tokens 足够支撑基本的追问场景
 * - 避免预算过小导致线程回答质量下降
 */
const THREAD_BUDGET_MIN = 2000;

/**
 * 线程 token 预算上限
 *
 * 为什么设置上限？
 * - 线程设计用于简短追问，不需要大量 token
 * - 防止单个线程占用过多预算，影响其他线程
 * - 8000 tokens 足够处理复杂的追问场景
 */
const THREAD_BUDGET_MAX = 8000;

// ============================================================
// 动态 Token 预算计算
// ============================================================

/**
 * 计算动态线程 token 预算
 *
 * Phase 6 优化：根据主对话的 token 预算动态计算线程预算。
 * 相比固定 3000 tokens，动态预算能更好地适应不同模型：
 * - 小模型（如 gpt-4o-mini，预算 128K）：线程预算 = min(128000 * 0.3, 8000) = 8000
 * - 大模型（如 claude-sonnet-4.5，预算 200K）：线程预算 = min(200000 * 0.3, 8000) = 8000
 * - 超大模型（预算 1M）：线程预算 = min(1000000 * 0.3, 8000) = 8000（上限保护）
 *
 * 计算公式：
 * 1. 基础预算 = totalTokens * THREAD_BUDGET_RATIO (30%)
 * 2. 下限保护 = max(基础预算, THREAD_BUDGET_MIN) (最少 2000)
 * 3. 上限保护 = min(下限保护, THREAD_BUDGET_MAX) (最多 8000)
 *
 * @param {number} totalTokens - 主对话可用总 token 数
 * @returns {number} 线程 token 预算（范围：2000 - 8000）
 *
 * @example
 * computeDynamicThreadBudget(100000) // => 8000 (30000 被上限截断)
 * computeDynamicThreadBudget(30000)  // => 9000 -> 8000 (被上限截断)
 * computeDynamicThreadBudget(10000)  // => 3000 -> 3000 (在范围内)
 * computeDynamicThreadBudget(5000)   // => 1500 -> 2000 (被下限提升)
 */
export function computeDynamicThreadBudget(totalTokens: number): number {
  // 第 1 步：计算基础预算（总预算的 30%）
  const baseBudget = Math.round(totalTokens * THREAD_BUDGET_RATIO);

  // 第 2 步：应用下限保护（最少 2000 tokens）
  const minProtected = Math.max(baseBudget, THREAD_BUDGET_MIN);

  // 第 3 步：应用上限保护（最多 8000 tokens）
  const finalBudget = Math.min(minProtected, THREAD_BUDGET_MAX);

  return finalBudget;
}

// ============================================================
// 哈希函数
// ============================================================

/**
 * cyrb53 非加密哈希函数
 *
 * 为什么选择 cyrb53？
 * - 快速：~10 行代码，性能优异
 * - 分布均匀：碰撞概率极低
 * - 确定性：相同输入始终产生相同输出
 * - 非加密：不需要加密安全性，只需要指纹识别
 *
 * 算法来源：https://github.com/bryc/code/blob/master/jshash/README.md#cyrb53
 *
 * @param {string} str - 待哈希的字符串
 * @param {number} [seed=1] - 随机种子（默认 1，用于产生不同的哈希序列）
 * @returns {number} 53 位整数哈希值（JavaScript 安全整数范围内）
 *
 * @example
 * cyrb53('hello') // => 2655442328
 * cyrb53('hello', 1) // => 2655442328 (相同 seed 相同结果)
 * cyrb53('hello', 2) // => 1762042571 (不同 seed 不同结果)
 */
export function cyrb53(str: string, seed = 1): number {
  // 初始化两个哈希值（seed 调节初始状态，增加哈希多样性）
  let h1 = 0xdeadbeef ^ seed;  // 第一个哈希分量（十六进制魔法数字，增加随机性）
  let h2 = 0x41c6ce57 ^ seed;  // 第二个哈希分量（与 h1 不同，避免对称）

  // 遍历字符串每个字符，更新哈希值
  for (let i = 0, ch; i < str.length; i++) {
    ch = str.charCodeAt(i);  // 获取字符的 Unicode 码点（0-65535）

    // 第一个哈希分量更新算法
    // 公式：h1 = Math.imul(h1 ^ ch, 2654435761)
    // - ^ ch：将字符码异或到当前哈希值（混淆）
    // - Math.imul：32 位整数乘法（C 风格，避免浮点误差）
    // - 2654435761：黄金比例相关的乘数（2^32 / φ ≈ 2654435761），使哈希分布更均匀
    h1 = Math.imul(h1 ^ ch, 2654435761);

    // 第二个哈希分量更新算法（使用不同的乘数）
    // - 1597334677：另一个黄金比例相关的乘数
    h2 = Math.imul(h2 ^ ch, 1597334677);
  }

  // 最终哈希值：合并两个分量并添加位置混淆
  // 公式：h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ...
  // - >>> 16：右移 16 位，交换高低位（增加扩散）
  // - 再次异或和乘法，使输出更均匀
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507);
  h1 = Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507);
  h2 = Math.imul(h2 ^ (h2 >>> 13), 3266489909);

  // 合并两个 32 位哈希值为一个 53 位整数
  // - (h2 >>> 0) 确保 h2 是无符号 32 位整数
  // - + 运算产生 53 位结果（在 JavaScript 安全整数范围内）
  return 4294967296 * (2097151 & h2) + (h1 >>> 0);
}

// ============================================================
// 指纹计算
// ============================================================

/**
 * 计算文本内容的指纹（用于匹配 DOM 元素）
 *
 * 指纹设计：
 * - 原始文本内容 + 起始偏移量 → cyrb53 哈希
 * - 包含偏移量确保：相同文本在不同位置有不同的指纹
 * - 例如：摘要中出现两次 "the method"，两次 "the method" 有不同指纹
 *
 * 为什么不只用文本内容？
 * - 如果同一段落出现多次，仅用文本无法区分是哪一次
 * - 包含位置信息（startOffset）可以精确定位
 *
 * @param {string} rawSource - 原始 markdown 源文本
 * @param {number} startOffset - 文本在源内容中的起始偏移量（字符位置）
 * @returns {string} 计算出的哈希指纹（字符串形式）
 *
 * @example
 * const md = "# Introduction\n\nThis is a method.";
 * computeFingerprint(md, 14) // => "This is a method." 的指纹
 */
export function computeFingerprint(rawSource: string, startOffset: number): string {
  // 拼接原始文本和起始偏移量，确保相同文本在不同位置有不同指纹
  const fingerprintInput = rawSource + startOffset;

  // 计算 cyrb53 哈希并转为字符串
  // 字符串格式便于存储和比较（JavaScript 数字和 JSON 兼容）
  return String(cyrb53(fingerprintInput));
}

// ============================================================
// 线程工厂函数
// ============================================================

/**
 * 创建一个新的线程对象
 *
 * 线程数据结构：
 * - id: 线程唯一标识（用于 React key 和更新定位）
 * - anchorFingerprint: 锚定元素的指纹（用于 DOM 匹配）
 * - anchorText: 用户选中的原文（用于显示）
 * - orphaned: 失效标记（原始消息重新生成后设为 true）
 * - messages: 线程内的消息数组
 *
 * 设计说明：
 * - 多个线程可以共享同一个 anchorFingerprint（同一段落多次追问）
 * - thread.id 提供唯一性，anchorFingerprint 只负责 DOM 匹配
 *
 * @param {string} fingerprint - 锚定元素的指纹（由 computeFingerprint 计算）
 * @param {string} anchorText - 用户选中的原文文本（用于显示和上下文）
 * @returns {Object} 新创建的线程对象
 *
 * @example
 * const thread = createThread('abc123', 'the method uses attention');
 * // => { id: '...', anchorFingerprint: 'abc123', anchorText: '...', orphaned: false, messages: [] }
 */
export function createThread(fingerprint: string, anchorText: string) {
  return {
    // 生成唯一线程 ID（时间戳 + 随机数）
    // 时间戳确保唯一性，随机数防止快速创建时碰撞
    id: Date.now().toString(36) + Math.random().toString(36).slice(2),

    // 锚定元素的指纹（用于 DOM 匹配）
    anchorFingerprint: fingerprint,

    // 用户选中的原文（用于线程头部显示）
    anchorText: anchorText,

    // 失效标记：原始消息重新生成后设为 true
    orphaned: false,

    // 线程内的消息数组（初始为空）
    messages: [],
  };
}

// ============================================================
// 线程消息操作
// ============================================================

/**
 * 向线程添加一条消息
 *
 * 用途：
 * - 用户发送追问后添加用户消息
 * - AI 回复后添加 AI 消息
 *
 * 设计说明：
 * - 生成新消息 ID（用于流式更新定位）
 * - 不可变操作：返回新线程对象而非修改原对象
 * - 实际使用时配合 updateThreadInMessages 进行深层更新
 *
 * @param {Object} thread - 原始线程对象
 * @param {Object} message - 要添加的消息对象 { id, role, content }
 * @returns {Object} 包含新消息的新线程对象
 *
 * @example
 * const updatedThread = addMessageToThread(thread, { role: 'user', content: 'Why?' });
 */
export function addMessageToThread(thread: Record<string, unknown>, message: { id?: string; role: string; content: string }) {
  return {
    ...thread,  // 复制线程对象（浅拷贝）
    messages: [
      ...(thread.messages as any[]),  // 保留原有消息
      {
        // 消息 ID：使用传入的 ID 或生成新 ID
        id: message.id || Date.now().toString(36) + Math.random().toString(36).slice(2),
        role: message.role,  // 'user' 或 'assistant'
        content: message.content,  // 消息内容（字符串或多模态数组）
      },
    ],
  };
}

/**
 * 更新线程中最后一条消息的内容（用于流式更新）
 *
 * 用途：
 * - AI 流式回复时，增量更新最后一条 assistant 消息的内容
 * - 每次收到 SSE 数据块时调用，累加内容
 *
 * 设计说明：
 * - 只更新最后一条消息（线程内最后一条必定是正在流式的 AI 消息）
 * - 不可变操作：返回新线程对象
 * - 如果线程无消息，直接返回原线程（防御性编程）
 *
 * @param {Object} thread - 原始线程对象
 * @param {string} content - 新的消息内容（累加后的完整内容）
 * @returns {Object} 更新后的线程对象
 *
 * @example
 * // 流式过程中，每次收到数据块
 * let streamingThread = thread;
 * streamingThread = updateLastThreadMessageContent(streamingThread, 'Hello');
 * streamingThread = updateLastThreadMessageContent(streamingThread, 'Hello world');
 */
export function updateLastThreadMessageContent(thread: Record<string, any>, content: string) {
  // 防御性检查：无消息时直接返回原线程
  if (!thread.messages || thread.messages.length === 0) {
    return thread;
  }

  // 获取最后一条消息的索引
  const lastIdx = thread.messages.length - 1;

  return {
    ...thread,  // 复制线程对象
    messages: (thread.messages as any[]).map((msg: any, idx: any) =>
      // 只更新最后一条消息
      idx === lastIdx
        ? { ...msg, content: content }  // 更新内容（保留 id 和 role）
        : msg  // 其他消息不变
    ),
  };
}

// ============================================================
// 深层不可变更新
// ============================================================

/**
 * 在 messages 数组中更新指定消息的指定线程（深层不可变更新）
 *
 * 为什么需要这个函数？
 * - React 状态要求不可变更新（不能直接修改对象）
 * - 线程嵌套在 messages[msgIdx].threads 数组中
 * - 需要精确更新某个线程，同时保持其他消息和线程的引用不变
 *
 * 设计要点：
 * - 使用 targeted spread 而非 structuredClone
 * - 只浅拷贝需要变更的层级，其他层级引用不变
 * - 确保 React.memo 正常工作（未变更组件不会重新渲染）
 *
 * @param {Array} messages - 完整的消息数组
 * @param {number} msgIdx - 要更新的消息索引
 * @param {string} threadId - 要更新的线程 ID
 * @param {Function} updater - 更新函数，接收线程对象，返回新线程对象
 * @returns {Array} 更新后的新消息数组
 *
 * @example
 * const newMessages = updateThreadInMessages(
 *   messages,
 *   2,                    // 第 3 条消息（索引 2）
 *   'thread-abc123',      // 线程 ID
 *   (thread) => ({ ...thread, orphaned: true })  // 标记为失效
 * );
 */
export function updateThreadInMessages(messages: Record<string, any>[], msgIdx: number, threadId: string, updater: (t: Record<string, any>) => Record<string, any>) {
  // 第一步：浅拷贝目标消息对象（只拷贝一层，threads 引用不变）
  const newMsg = { ...messages[msgIdx] };

  // 第二步：更新 threads 数组中的目标线程
  // - map 创建新数组（只拷贝一层，非目标线程引用不变）
  // - 找到目标线程时调用 updater 更新
  newMsg.threads = (newMsg.threads ?? []).map((t: any) =>
    t.id === threadId ? updater({ ...t }) : t  // updater 接收线程拷贝，返回新线程
  );

  // 第三步：创建新的 messages 数组
  // - 只拷贝一层，非目标消息引用不变
  // - 使用 map 替代手动遍历，代码更简洁
  return messages.map((m, i) => (i === msgIdx ? newMsg : m));
}

// ============================================================
// 线程失效标记
// ============================================================

/**
 * 将指定消息的所有线程标记为失效（orphaned）
 *
 * 使用场景：
 * - 用户点击"重新生成"按钮，AI 重新生成回复
 * - 原回复的文本内容完全改变，原有线程的锚定失效
 * - 标记为失效而非删除，保留用户的追问历史
 *
 * 失效线程的 UI 行为：
 * - 头部显示提示："原始回答已变更，AI 将基于最新回答回答你的追问"
 * - 使用灰色虚线边框区分（视觉降级）
 * - 用户可继续在失效线程中追问（AI 会重新获得完整上下文）
 * - 用户可手动删除失效线程
 *
 * @param {Array} messages - 完整的消息数组
 * @param {number} msgIdx - 要标记的消息索引
 * @returns {Array} 更新后的新消息数组
 *
 * @example
 * const newMessages = markThreadsOrphaned(messages, 2);
 */
export function markThreadsOrphaned(messages: Record<string, any>[], msgIdx: number) {
  // 浅拷贝目标消息对象
  const newMsg = { ...messages[msgIdx] };

  // 将所有线程标记为失效（创建新 threads 数组）
  newMsg.threads = (newMsg.threads ?? []).map((t: any) => ({
    ...t,
    orphaned: true,  // 设置失效标记
  }));

  // 创建新的 messages 数组（只替换目标消息）
  return messages.map((m, i) => (i === msgIdx ? newMsg : m));
}

// ============================================================
// 线程结构规范化
// ============================================================

/**
 * 确保所有消息的 threads 字段都是数组（规范化旧数据）
 *
 * 为什么需要？
 * - 旧消息可能没有 threads 字段（null 或 undefined）
 * - 旧消息可能 threads 是空数组（已经是正确格式）
 * - 统一处理避免消费侧分散判断 `msg.threads || []`
 *
 * 设计说明：
 * - 不可变操作：返回新 messages 数组
 * - 只规范化 threads 字段，不修改其他字段
 * - 如果 threads 已经是数组，直接保留（性能优化）
 *
 * @param {Array} messages - 原始消息数组
 * @returns {Array} 规范化后的消息数组
 *
 * @example
 * const oldMessages = [{ role: 'assistant', content: '...' }];  // 无 threads 字段
 * const normalized = ensureThreadStructure(oldMessages);
 * // => [{ role: 'assistant', content: '...', threads: [] }]
 */
export function ensureThreadStructure(messages: Record<string, any>[]) {
  return messages.map((msg) => {
    // 如果 threads 已经是数组，直接返回原消息（无变更）
    if (Array.isArray(msg.threads)) {
      return msg;
    }

    // 否则创建新消息对象，threads 设为空数组
    return {
      ...msg,
      threads: [],
    };
  });
}

// ============================================================
// 子线程结构规范化
// ============================================================

/**
 * 规范化线程消息的 threads 字段（子线程支持）
 *
 * 为什么需要？
 * - 子线程嵌套在 assistant 线程消息中，旧数据可能没有 threads 字段
 * - 确保消费侧无需分散判断 `msg.threads || []`
 * - 只处理 assistant 消息，user 消息不会有子线程
 *
 * @param {Object} threadMsg - 线程消息对象
 * @returns {Object} 规范化后的线程消息对象
 */
export function ensureSubThreadStructure(threadMsg: Record<string, any>) {
  if (threadMsg.role !== 'assistant') {
    return threadMsg;
  }

  if (!Array.isArray(threadMsg.threads)) {
    return {
      ...threadMsg,
      threads: [],
    };
  }

  return threadMsg;
}

// ============================================================
// 子线程深层不可变更新
// ============================================================

/**
 * 在 messages 数组中更新指定子线程（二级深度不可变更新）
 *
 * 这是 updateThreadInMessages 的子线程版本，更新路径为：
 * messages[msgIdx].threads[parentId].messages[assistant with threads].threads[subThreadId]
 *
 * 设计要点：
 * - 三层 spread：消息层 → 父线程层 → assistant 消息层 → 子线程层
 * - 只拷贝变更路径上的层级，其他引用不变
 * - 确保 React.memo 正常工作
 *
 * @param {Array} messages - 完整的消息数组
 * @param {number} msgIdx - 主消息索引
 * @param {string} threadId - 父线程 ID
 * @param {string} subThreadId - 子线程 ID
 * @param {Function} updater - 更新函数，接收子线程对象，返回新子线程对象
 * @returns {Array} 更新后的新消息数组
 */
export function updateSubThreadInMessages(messages: Record<string, any>[], msgIdx: number, threadId: string, subThreadId: string, updater: (t: Record<string, any>) => Record<string, any>) {
  const newMsg = { ...messages[msgIdx] };

  newMsg.threads = (newMsg.threads ?? []).map((parentThread: any) => {
    if (parentThread.id !== threadId) return parentThread;

    return {
      ...parentThread,
      messages: (parentThread.messages as any[]).map((threadMsg: any) => {
        if (!Array.isArray(threadMsg.threads)) return threadMsg;

        const subIdx = (threadMsg.threads as any[]).findIndex((st: any) => st.id === subThreadId);
        if (subIdx === -1) return threadMsg;

        const newThreads = [...threadMsg.threads];
        newThreads[subIdx] = updater({ ...newThreads[subIdx] });

        return { ...threadMsg, threads: newThreads };
      }),
    };
  });

  return messages.map((m, i) => (i === msgIdx ? newMsg : m));
}

// ============================================================
// 线程系统提示词构建
// ============================================================
//
// Phase 6 架构定位：
// - 本模块的 buildThreadSystemPrompt 是独立的 prompt 路径，输出 string 格式
// - 与主对话路径（systemPromptBuilder.buildSystemPromptBlocks，输出 CacheControlBlock[]）并行存在
// - strategies/ThreadStrategy.ts 描述了"未来统一"的目标 layer 配置
// - 当前不强制 unification 的原因：
//   1. byte-level 测试覆盖（threadUtils.test.ts:503-553）—— 改格式需要同步更新
//   2. useChatSender 的 dispatch：thread 走 /api/ai/proxy，main 走 /api/agent/chat
//      endpoint 差异是另一个独立问题，不在 prompt 路径 unification 范围内
//   3. 风险/回报不平衡：当前路径工作良好，unification 收益主要是架构整洁

/**
 * 构建线程追问的系统提示词
 *
 * 设计目标：
 * - 告知 AI 用户正在针对之前回答中的特定段落进行追问
 * - 展示用户选中的原文（anchorText）作为焦点
 * - 要求 AI 简洁回答（线程 UI 空间有限）
 * - 提供 AI 回复全文作为参考（截取到预算内）
 *
 * 提示词结构：
 * 1. 说明这是追问场景
 * 2. 展示用户选中的原文
 * 3. 提供 AI 原始回复上下文（截取）
 * 4. 要求简洁回答
 *
 * @param {Object} threadContext - 线程上下文对象
 * @param {string} threadContext.anchorText - 用户选中的原文
 * @param {string} [threadContext.assistantReply] - AI 原始回复（用于上下文）
 * @param {number} [dynamicBudget] - 动态 token 预算（Phase 6：如果提供则使用，否则使用固定值）
 * @returns {string} 线程系统提示词
 *
 * @example
 * const threadContext = { anchorText: 'the method uses attention', assistantReply: '...' };
 * const prompt = buildThreadSystemPrompt(threadContext, 5000);
 */
export function buildThreadSystemPrompt(threadContext: { anchorText?: string; assistantReply?: string }, dynamicBudget: number | null = null): string {
  const { anchorText, assistantReply } = threadContext;

  // Phase 6: 使用动态预算（如果提供），否则使用固定预算
  const threadBudget = dynamicBudget !== null ? dynamicBudget : THREAD_TOKEN_BUDGET;

  // 截取 anchorText（超出 ANCHOR_TEXT_MAX_TOKENS 时截断）
  // 估算：1 token ≈ 4 字符（粗略估算，适用于英文）
  // 中文场景：1 token ≈ 1.5 字符，这里使用保守估算
  let truncatedAnchorText = anchorText || '';
  const maxChars = ANCHOR_TEXT_MAX_TOKENS * 4;  // 转换为字符数

  if (truncatedAnchorText.length > maxChars) {
    truncatedAnchorText = truncatedAnchorText.slice(0, maxChars) + '...[已截断]';
  }

  // 构建提示词（使用模板字符串，格式清晰）
  let systemPrompt = `【追问模式】用户正在针对你之前回答中的特定段落进行追问。

用户选中的原文：
"""
${truncatedAnchorText}
"""`;

  // 如果有 AI 原始回复，添加上下文（截取到预算内）
  if (assistantReply) {
    // Phase 6: 使用动态预算计算剩余空间
    // 估算剩余 token 预算（扣除 Layer 1 约 500 tokens，anchorText 约 1000 tokens）
    const remainingBudget = threadBudget - 1500;
    const maxReplyChars = remainingBudget * 4;

    let truncatedReply = assistantReply;
    if (assistantReply.length > maxReplyChars) {
      truncatedReply = assistantReply.slice(0, maxReplyChars) + '...[已截断]';
    }

    systemPrompt += `

你的原始回复（作为参考）：
"""
${truncatedReply}
"""`;
  }

  // 添加回答要求
  // v2 (2026-06-28): 删除「可引用上下文」模糊指引（assistantReply 已注入，无需 prompt 说）；
  // 加 P7 边界（选中文本不足以回答时如何降级，避免 LLM 硬猜）
  systemPrompt += `

【回答要求】
1. 简洁直接：追问显示在内联线程中，空间有限，直接回答不要绕弯。
2. 聚焦选中文本：优先解释用户选中的原文段落。
3. 边界：若选中文本本身信息不足以回答追问（如需要论文其他章节、需要外部资料），明确说明需要哪些额外信息，不要硬猜。`;

  return systemPrompt;
}

// ============================================================
// 子线程系统提示词构建
// ============================================================

/**
 * 构建子线程（二级追问）的系统提示词
 *
 * 设计目标：
 * - 告知 AI 用户正在针对追问线程中 AI 回复的特定段落进行继续追问
 * - 展示一级追问的原文和二级追问选中的 AI 回复段落
 * - 注入完整父线程对话历史（不截断，目标 LLM 百万 token 上下文）
 * - 要求 AI 简洁回答，聚焦选中文本
 *
 * 与 buildThreadSystemPrompt 的区别：
 * - 不设 token 预算限制（目标 LLM 百万 token）
 * - 包含完整父线程对话历史
 * - 包含一级追问和二级追问两层上下文
 *
 * @param {Object} subThreadContext - 子线程上下文对象
 * @param {string} subThreadContext.anchorText - 子线程选中的 AI 回复段落
 * @param {string} subThreadContext.parentAnchorText - 父线程的 anchorText（一级追问的原文）
 * @param {string} subThreadContext.assistantReply - 被追问的 assistant ThreadMessage 完整内容
 * @param {Array} parentThreadMessages - 父线程内全部消息数组（完整注入，不截断）
 * @param {Object} paperInfo - 论文信息对象（用于 Layer 1 上下文）
 * @returns {string} 子线程系统提示词
 */
export function buildSubThreadSystemPrompt(subThreadContext: { anchorText?: string; parentAnchorText?: string; assistantReply?: string }, parentThreadMessages: Array<Record<string, any>>, paperInfo: PaperContextInfo | null): string {
  const { anchorText: subThreadAnchorText, parentAnchorText, assistantReply } = subThreadContext;

  // 构建父线程对话历史（逐条格式化为 User: ... / Assistant: ...）
  const conversationHistory = parentThreadMessages
    .map((msg) => {
      const role = msg.role === 'user' ? 'User' : 'Assistant';
      const content = typeof msg.content === 'string' ? msg.content : '[非文本内容]';
      return `${role}: ${content}`;
    })
    .join('\n\n');

  let systemPrompt = `【二级追问模式】用户正在针对追问线程中 AI 回复的特定段落进行继续追问。

一级追问的原文：
"""
${parentAnchorText || ''}
"""

二级追问选中的 AI 回复段落：
"""
${subThreadAnchorText || ''}
"""`;

  // 注入完整父线程对话历史
  if (conversationHistory) {
    systemPrompt += `

追问线程完整对话历史：
${conversationHistory}`;
  }

  // 添加回答要求
  // v2 (2026-06-28): 加 P7 边界（选中文本不足以回答时如何降级）
  systemPrompt += `

【回答要求】
1. 简洁直接：二级追问空间更有限，直接回答不要绕弯。
2. 聚焦选中文本：优先解释用户选中的 AI 回复段落。
3. 结合父线程上下文：参考上方完整对话历史。
4. 边界：若选中文本不足以回答（如需要重新查阅一级追问原文、需要外部资料），明确说明需要哪些额外信息，不要硬猜。`;

  // 如果有论文信息， prepend 论文上下文（与 buildThreadSystemPrompt 保持一致）
  const paperContext = formatPaperContext(paperInfo);
  if (paperContext) {
    return paperContext + '\n\n' + systemPrompt;
  }

  return systemPrompt;
}

// ============================================================
// 指纹匹配验证（anchorText 文本匹配兜底）
// ============================================================

/**
 * 当指纹匹配到多个候选块时，用原文做二次验证
 *
 * 设计目标：
 * - 解决指纹冲突问题：同一段落出现多次时，指纹无法区分
 * - 使用 anchorText 与候选块的原文进行子串匹配，找到正确的块
 * - 提升线程锚定的准确性，减少误匹配
 *
 * 使用场景：
 * - remarkThreadAnchor 插件生成多个相同指纹的块时
 * - MarkdownRenderer 渲染线程时需要精确匹配锚定位置
 *
 * @param {string} fingerprint - 目标指纹（线程的 anchorFingerprint）
 * @param {Array<{fingerprint: string, start: number, end: number}>} candidates - 候选块数组
 * @param {string} content - 完整 markdown 源文本
 * @param {string} anchorText - 线程的锚定文本（用户选中的原文）
 * @returns {string|null} 匹配的 fingerprint 或 null（无匹配时）
 *
 * @example
 * // 摘要中出现两次 "attention mechanism"
 * const candidates = [
 *   { fingerprint: 'abc123', start: 100, end: 120 },
 *   { fingerprint: 'abc123', start: 300, end: 320 },
 * ];
 * const content = 'The attention mechanism is... Later, the attention mechanism...';
 * const result = verifyFingerprintWithText('abc123', candidates, content, 'attention mechanism');
 * // 返回第一个匹配的候选块的 fingerprint
 */
export function verifyFingerprintWithText(fingerprint: string, candidates: Array<{fingerprint: string; start: number; end: number}>, content: string, anchorText: string): string | null {
  // 第 1 步：如果只有一个候选，直接返回
  if (candidates.length === 0) {
    return null;
  }
  if (candidates.length === 1) {
    return candidates[0].fingerprint;
  }

  // 第 2 步：多个候选时，用 content.slice(start, end) 与 anchorText 做子串匹配
  const normalizedAnchor = anchorText?.trim().toLowerCase() || '';

  // 如果 anchorText 为空，返回第一个候选（降级处理）
  if (!normalizedAnchor) {
    return candidates[0].fingerprint;
  }

  // 遍历所有候选，查找最佳匹配
  for (const candidate of candidates) {
    // 提取候选块的原文
    const blockText = content.slice(candidate.start, candidate.end).trim().toLowerCase();

    // 判断 anchorText 是否是 blockText 的子串
    if (blockText.includes(normalizedAnchor)) {
      return candidate.fingerprint;
    }
  }

  // 第 3 步：无匹配时，使用模糊匹配（包含最多关键词的候选）
  let bestMatch = null;
  let maxOverlap = 0;

  for (const candidate of candidates) {
    const blockText = content.slice(candidate.start, candidate.end).trim().toLowerCase();
    // 计算重叠字符数
    let overlap = 0;
    for (let i = 0; i < Math.min(normalizedAnchor.length, blockText.length); i++) {
      if (normalizedAnchor[i] === blockText[i]) {
        overlap++;
      } else {
        break;
      }
    }

    if (overlap > maxOverlap) {
      maxOverlap = overlap;
      bestMatch = candidate.fingerprint;
    }
  }

  return bestMatch || candidates[0].fingerprint;
}

// ============================================================
// 失效线程自动恢复
// ============================================================

/**
 * 消息重新生成后，基于 anchorText 模糊匹配尝试恢复线程锚定
 *
 * 设计目标：
 * - 用户点击"重新生成"后，AI 回复内容完全改变
 * - 原有线程的 fingerprint 匹配失败，被标记为 orphaned
 * - 本函数尝试在新内容中查找 anchorText，恢复线程锚定
 * - 减少失效线程数量，提升用户体验
 *
 * 使用场景：
 * - 消息重新生成后调用
 * - 检测该消息的所有 orphaned 线程
 * - 对每个线程，检查 anchorText 是否是新 content 的子串
 * - 如果是，用新位置重新计算 fingerprint，设置 orphaned=false
 *
 * @param {Array} messages - 消息数组
 * @param {number} msgIdx - 重新生成的消息索引
 * @param {string} newContent - 新的消息内容（markdown 格式）
 * @returns {Array} 更新后的消息数组（恢复的线程 orphaned=false，重新计算 fingerprint）
 *
 * @example
 * const newMessages = recoverOrphanedThreads(messages, 2, newAiResponse);
 * // orphaned 线程的 fingerprint 被更新，orphaned=false
 */
export function recoverOrphanedThreads(messages: Record<string, any>[], msgIdx: number, newContent: string) {
  // 防御性检查：消息索引越界
  if (msgIdx < 0 || msgIdx >= messages.length) {
    return messages;
  }

  const msg = messages[msgIdx];
  if (!msg.threads || msg.threads.length === 0) {
    return messages; // 无线程，无需处理
  }

  // 获取该消息的所有 orphaned 线程
  const orphanedThreads = (msg.threads as any[]).filter((t: any) => t.orphaned);
  if (orphanedThreads.length === 0) {
    return messages; // 无 orphaned 线程，无需处理
  }

  // 对每个 orphaned 线程，尝试恢复锚定
  const updatedThreads = (msg.threads as any[]).map((thread: any) => {
    // 只处理 orphaned 线程
    if (!thread.orphaned) return thread;

    // 检查 anchorText 是否是新 content 的子串
    const anchorText = thread.anchorText?.trim() || '';
    if (!anchorText) return thread; // 无 anchorText，无法恢复

    // 在新内容中查找 anchorText
    const anchorIndex = newContent.toLowerCase().indexOf(anchorText.toLowerCase());
    if (anchorIndex === -1) {
      // 未找到，保持 orphaned 状态
      return thread;
    }

    // 找到了！重新计算 fingerprint，恢复线程
    const newFingerprint = computeFingerprint(newContent, anchorIndex);

    return {
      ...thread,
      anchorFingerprint: newFingerprint,
      orphaned: false, // 恢复正常状态
    };
  });

  // 更新消息数组（不可变）
  const newMsg = { ...msg, threads: updatedThreads };
  return messages.map((m, i) => (i === msgIdx ? newMsg : m));
}

// ============================================================
// 线程发送上下文构建（从 useChatSender 抽离，纯函数）
// ============================================================

/** 主对话 token 预算回退值（settings 里没配 default_budget 时使用） */
export const DEFAULT_TOKEN_BUDGET = 150000;
/** 线程 token 用量超过预算的该比例时静默告警（当前仅留作未来日志钩子） */
export const BUDGET_WARNING_RATIO = 0.8;

/** 线程上下文（msgIdx 定位父消息，threadId/subThreadId 定位线程层级） */
export interface ThreadContext {
  msgIdx: number;
  threadId: string;
  subThreadId?: string;
  userMessageId?: string;
  assistantMessageId?: string;
  threadSystemPrompt?: string;
  anchorText?: string;
}

/** 线程发送选项（外部传入，控制流和回调） */
export interface ThreadOptions {
  abortController?: AbortController;
  threadContext?: ThreadContext;
  onStreamUpdate?: (updaterOrValue: unknown) => void;
  onStreamComplete?: () => void;
  onStreamError?: (error: Error) => void;
}

/** 主对话 token 预算配置 */
export interface TokenBudgets {
  budgets?: Record<string, number>;
  default_budget?: number;
}

/** prepareThreadContext 成功时的返回值 */
export interface PreparedThreadContext {
  isThreadMode: true;
  threadSystemPrompt: string;
  threadApiMessages: Array<{ role: string; content: unknown }>;
  threadTokenBudget: number;
}

/**
 * 聊天消息（线程上下文构建只需要 role/content/threads 字段）。
 *
 * 注：本接口对 ChatMessageBase（@/types/chat）的 role/content 做了 widen
 * （base 是字面量联合 + string|unknown[]，本接口是 string + unknown），
 * 因此不能用 extends 表达。widen 是有意的——prepareThreadContext 只读
 * 这几个字段，且需要接受 store / 各 hook 的 ChatMessage 变体作为入参。
 */
interface ChatMessageLike {
  id?: string;
  role: string;
  content: unknown;
  threads?: unknown[];
}

/**
 * 准备线程上下文
 *
 * 线程模式（threadOptions 不为 null 且包含 threadContext）时：
 * 1. 根据当前模型的主对话预算动态算出 threadTokenBudget（main × 0.3，clamp 2000-8000）
 * 2. 构建线程系统提示词（threadContext.threadSystemPrompt 优先，否则按 anchorText 现算）
 * 3. 从父消息的 threads 数组里取出 threadContext.threadId 对应线程的消息作为上下文
 *    - 子线程（subThreadId 存在）：定位 subThreadId 对应的子线程消息
 *    - 一级线程：直接用父线程消息
 *    - 取最近 10 条作为对话历史
 * 4. 拼出 threadApiMessages：[system?] + 历史 + 当前用户消息
 *
 * 非线程模式返回 null，调用方按主对话流程处理。
 */
export function prepareThreadContext({
  threadOptions,
  currentModel,
  settings,
  tokenBudgets,
  messages,
  userMessage,
}: {
  threadOptions: ThreadOptions | null;
  currentModel: ModelConfig | null;
  settings: Record<string, unknown>;
  tokenBudgets: TokenBudgets | Record<string, number> | null;
  messages: ChatMessageLike[];
  userMessage: string;
}): PreparedThreadContext | null {
  const isThreadMode = threadOptions !== null;
  const threadContext = threadOptions?.threadContext || null;

  if (!isThreadMode || !threadContext) {
    return null;
  }

  let threadSystemPrompt = '';
  let threadApiMessages: Array<{ role: string; content: unknown }> = [];
  let threadTokenBudget = THREAD_TOKEN_BUDGET;

  // 动态计算线程 token 预算
  try {
    const customConfig = currentModel?.key === 'custom' ? currentModel.config : null;
    const modelConfig = JSON.parse((settings.model_config as string) || '{}');
    const model = customConfig?.modelName || modelConfig.model || settings.model || 'gpt-4o-mini';

    const budgets = (tokenBudgets as TokenBudgets | undefined)?.budgets || tokenBudgets;
    const defaultBudget = (tokenBudgets as TokenBudgets | undefined)?.default_budget || DEFAULT_TOKEN_BUDGET;
    const mainTokenBudget = (budgets as Record<string, number>)[model] || defaultBudget;

    threadTokenBudget = computeDynamicThreadBudget(mainTokenBudget);
  } catch {
    threadTokenBudget = THREAD_TOKEN_BUDGET;
  }

  // 构建线程系统提示词
  if (threadContext.threadSystemPrompt) {
    threadSystemPrompt = threadContext.threadSystemPrompt;
  } else {
    threadSystemPrompt = buildThreadSystemPrompt({
      anchorText: threadContext.anchorText || '',
      assistantReply: '',
    }, threadTokenBudget);
  }

  // 构建线程 API 消息数组
  const parentMsg = messages[threadContext.msgIdx];
  const parentThreads = (parentMsg?.threads ?? []) as Array<{ id?: string; messages?: Array<{ role: string; content: unknown; threads?: Array<{ id?: string; messages?: Array<{ role: string; content: unknown }> }> }> }>;
  const parentThread = parentThreads.find(t => t.id === threadContext.threadId);

  let contextMessages: Array<{ role: string; content: unknown }> = [];
  if (threadContext.subThreadId) {
    // 子线程模式：在父线程消息里找到挂载 subThreadId 的子线程，用其 messages
    for (const threadMsg of (parentThread?.messages ?? [])) {
      if (Array.isArray(threadMsg.threads)) {
        const subThread = threadMsg.threads.find(st => st.id === threadContext.subThreadId);
        if (subThread) {
          contextMessages = subThread.messages || [];
          break;
        }
      }
    }
  } else {
    // 一级线程模式：直接用父线程消息
    contextMessages = parentThread?.messages ?? [];
  }

  const recentThreadMessages = contextMessages.slice(-10);

  threadApiMessages = [];
  if (threadSystemPrompt) {
    threadApiMessages.push({ role: 'system', content: threadSystemPrompt });
  }

  recentThreadMessages.forEach((m) => {
    threadApiMessages.push({ role: m.role, content: m.content });
  });

  threadApiMessages.push({ role: 'user', content: userMessage });

  // Token 使用率警告（保留钩子，当前静默）
  let threadTokens = 0;
  threadApiMessages.forEach((m) => {
    if (m.role === 'system') return;
    const serialized = typeof m.content === 'string' ? m.content : JSON.stringify(m.content);
    threadTokens += estimateTokens(serialized);
  });

  const usageRatio = threadTokens / threadTokenBudget;
  if (usageRatio > BUDGET_WARNING_RATIO) {
    // Token usage is high, but we continue silently
  }

  return {
    isThreadMode: true,
    threadSystemPrompt,
    threadApiMessages,
    threadTokenBudget,
  };
}
