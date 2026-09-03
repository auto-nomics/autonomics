/**
 * 多模态消息工具函数
 *
 * 本模块提供 AI 聊天消息格式转换的纯函数工具集。
 * 处理 OpenAI 和 Anthropic 两种 API 的消息格式差异。
 * 所有函数都是纯函数，无副作用，便于测试和维护。
 *
 * @module ai-chat/utils/chatMessageUtils
 */

/**
 * 将 OpenAI 格式的多模态 parts 转换为 Anthropic 格式的 content blocks
 *
 * 两种 API 的图片格式差异：
 * - OpenAI: { type: 'image_url', image_url: { url: 'data:image/png;base64,...' } }
 * - Anthropic: { type: 'image', source: { type: 'base64', media_type: 'image/png', data: '...' } }
 *
 * 为什么需要这个转换？
 * - 两种 API 的多模态消息格式不同，需要根据服务商进行转换
 * - OpenAI 接受 Data URL 作为图片地址
 * - Anthropic 要求图片以 base64 原始数据 + media_type 的格式传入
 *
 * @param {string|Array} content - 消息内容（字符串或 OpenAI 格式的 parts 数组）
 * @returns {string|Array} 转换后的 Anthropic 格式内容
 */
export function convertToAnthropicContent(content: string | Array<Record<string, any>>): string | Array<Record<string, any>> {
  // 纯文本消息直接返回（字符串格式 Anthropic 也支持）
  if (typeof content === 'string') return content;
  // 非数组直接返回（防御性处理，避免意外类型导致崩溃）
  if (!Array.isArray(content)) return content;

  // Anthropic 格式的 content blocks 数组
  const blocks = [];

  // 遍历每个 part，将 OpenAI 格式转换为 Anthropic 格式
  for (const part of content) {
    if (part.type === 'text') {
      // 文本类型：直接添加为 Anthropic text block
      const text = part.text ?? ''; // 获取文本，null/undefined 转为空字符串
      if (text) { // 仅添加非空文本，避免无意义的空块
        blocks.push({ type: 'text', text }); // 添加 Anthropic 文本块
      }
    } else if (part.type === 'image_url' && part.image_url) {
      // 图片类型：将 OpenAI 的 image_url 格式转换为 Anthropic 的 image 格式
      // OpenAI: { type: 'image_url', image_url: { url: 'data:image/png;base64,...' } }
      // Anthropic: { type: 'image', source: { type: 'base64', media_type: '...', data: '...' } }

      // 提取图片 URL（兼容字符串和对象两种格式）
      const url = typeof part.image_url === 'string' ? part.image_url : part.image_url.url;
      if (!url) continue; // URL 为空则跳过该图片

      // 使用正则解析 Data URL 格式：data:<mediaType>;base64,<data>
      // 捕获组 1: MIME 类型（如 image/png、image/jpeg）
      // 捕获组 2: Base64 编码的图片数据
      const match = /^data:([^;]+);base64,(.+)$/i.exec(url);
      if (match) {
        // 解析成功，构建 Anthropic 格式的图片块
        blocks.push({
          type: 'image', // Anthropic 的图片块类型标识
          source: {
            type: 'base64', // 数据来源类型：base64 编码
            media_type: match[1] || 'image/png', // MIME 类型，解析失败时默认 png
            data: match[2].replace(/\s/g, ''), // Base64 数据，去除所有空白字符
          },
        });
      }
    } else if (part.type === 'image' && part.source) {
      // 已经是 Anthropic 格式的图片块（防御性处理），直接透传
      blocks.push(part);
    }
    // 其他未知类型的 part 直接跳过（避免引入无效数据）
  }

  // 优化：如果所有 blocks 都是文本类型（没有图片），合并为一个纯文本字符串
  // 这样可以减少 API 请求的 JSON 大小，也兼容不支持多模态的端点
  if (blocks.length > 0 && blocks.every(b => b.type === 'text')) {
    return blocks.map(b => b.text).join(''); // 拼接所有文本块为一个字符串
  }

  return blocks; // 返回包含图片和文本的 blocks 数组
}

/**
 * 构建用户消息的多模态内容
 *
 * 根据附件类型构建不同格式的消息内容：
 * - 纯文本 + 图片附件：返回 OpenAI 格式的 parts 数组（文本 + 图片）
 * - 纯文本 + 文件附件（文本/二进制）：文件内容注入到文本消息中
 * - 混合附件：图片作为 image_url parts，文件内容注入文本
 *
 * 附件类型说明：
 * - image: 图片附件，以 base64 Data URL 格式发送（多模态 API 支持）
 * - text: 文本文件附件，内容直接注入到消息文本中
 * - binary: 二进制文件附件（后端已转换为 Markdown），内容注入到消息文本中
 *
 * @param {string} text - 用户输入的文本内容
 * @param {Array} attachments - 附件数组（统一格式，包含 image/text/binary 三种类型）
 * @returns {string|Array} 消息内容（字符串或 parts 数组）
 */
export function buildUserMessageContent(text: string, attachments: Array<Record<string, any>> | undefined) {
  // 分离图片附件和文件附件（文本/二进制）
  const imageAttachments = [];  // 图片附件列表（发送为 image_url parts）
  const fileTextParts = [];     // 文件内容文本片段列表（注入到消息文本中）

  if (attachments && attachments.length > 0) {
    // 遍历所有附件，按类型分类处理
    for (const att of attachments) {
      if (att.category === 'image') {
        // 图片附件：收集到图片列表，后续生成 image_url parts
        if (att.base64) {
          imageAttachments.push(att);
        }
      } else if (att.category === 'text') {
        // 文本文件附件：将文件内容格式化后注入到消息文本中
        // 格式：--- 附件: 文件名 ---\n```语言\n内容\n```
        const lang = att.language || '';
        const codeFence = lang ? `\`\`\`${lang}` : '```';
        fileTextParts.push(
          `--- 附件: ${att.name} ---\n${codeFence}\n${att.textContent || ''}\n\`\`\`\n---`
        );
      } else if (att.category === 'binary' && att.status === 'ready') {
        // 二进制文件附件（后端已转换为 Markdown）：将转换后的内容注入到消息文本中
        if (att.markdown) {
          fileTextParts.push(
            `--- 附件: ${att.name} (已转换) ---\n${att.markdown}\n---`
          );
        }
      }
    }
  }

  // 构建完整的文本部分（用户输入 + 文件内容）
  let fullText = text || '';
  if (fileTextParts.length > 0) {
    // 将文件内容追加到用户文本之后
    fullText = fullText + '\n\n' + fileTextParts.join('\n\n');
  }

  // 如果没有图片附件，返回纯文本（可能包含文件内容）
  if (imageAttachments.length === 0) {
    return fullText;  // 纯文本消息（含文件内容）
  }

  // 有图片附件：构建 OpenAI 格式的多模态 parts 数组
  const parts = [];

  // 添加文本 part（如果有内容）
  if (fullText.trim()) {
    parts.push({ type: 'text', text: fullText });
  }

  // 为每张图片创建 image_url part
  for (const img of imageAttachments) {
    if (img.base64) {
      parts.push({
        type: 'image_url',
        image_url: {
          url: img.base64,  // Data URL 格式：data:image/png;base64,...
        },
      });
    }
  }

  // 如果 parts 只有图片没有文本，添加默认文本占位符
  // 部分 AI API 要求多模态消息至少包含一个文本 part
  if (parts.length > 0 && !parts.some(p => p.type === 'text')) {
    parts.unshift({ type: 'text', text: '[图片]' });
  }

  return parts;
}

/**
 * 从消息内容中移除附件数据，仅保留文本摘要（用于持久化存储）
 *
 * 为什么需要剥离附件数据？
 * - 图片的 base64 编码通常很大（每张图片数百 KB 到数 MB）
 * - 文件内容（尤其是转换后的 Markdown）也可能很长
 * - 聊天记录以 JSON 格式存储在 SQLite 数据库中
 * - 如果保存全部附件数据，数据库会迅速膨胀，影响查询性能
 * - 历史消息中保留附件的文本摘要即可，附件内容仅在当前会话中有效
 *
 * @param {string|Array} content - 原始消息内容（字符串或多模态 parts 数组）
 * @returns {string} 仅包含文本的消息内容（附件替换为摘要占位符）
 */
export function stripAttachmentsForPersistence(content: string | Array<Record<string, any>>): string {
  // 纯文本消息直接返回，无需处理
  if (typeof content === 'string') return content;
  // 非数组类型（null、undefined、数字等）转为字符串返回
  if (!Array.isArray(content)) return String(content ?? '');

  // 从多模态 parts 数组中提取所有文本内容
  const textParts = content
    .filter(part => part.type === 'text' && part.text)  // 过滤出非空的文本 part
    .map(part => {
      let text = part.text;
      // 将附件内容块替换为摘要占位符
      // 匹配格式：--- 附件: 文件名 ---\n内容\n---
      text = text.replace(
        /--- 附件: (.+?) ---\n[\s\S]*?\n---/g,
        '[附件: $1]'  // 替换为简洁的占位符，如 "[附件: report.pdf]"
      );
      return text;
    });

  // 拼接所有文本部分，用换行符分隔
  return textParts.join('\n') || '';
}

/**
 * 从消息内容中提取纯文本（用于聊天界面显示）
 *
 * 处理两种格式：
 * - 字符串：直接返回
 * - 数组（多模态）：提取所有 text 类型的 part 并拼接
 *
 * @param {string|Array} content - 消息内容
 * @returns {string} 提取的纯文本
 */
export function extractTextFromContent(content: string | Array<Record<string, any>> | null | undefined): string {
  // 字符串类型直接返回（最常见的纯文本消息）
  if (typeof content === 'string') return content;
  // 非数组类型转为字符串
  if (!Array.isArray(content)) return String(content ?? '');

  // 从多模态 parts 数组中提取所有文本 part 的内容
  return content
    .filter(part => part.type === 'text') // 过滤出文本类型的 part
    .map(part => part.text || '') // 提取文本，空值转为空字符串
    .join('\n'); // 用换行符拼接多个文本 part
}

/**
 * 从消息内容中提取所有图片 URL（用于聊天界面显示图片缩略图）
 *
 * @param {string|Array} content - 消息内容
 * @returns {Array<string>} 图片 URL 数组（Data URL 格式）
 */
export function extractImageUrlsFromContent(content: string | Array<Record<string, any>>): string[] {
  // 纯文本消息没有图片，返回空数组
  if (!Array.isArray(content)) return [];

  // 从 parts 数组中提取所有 image_url 类型的 URL
  return content
    .filter(part => part.type === 'image_url' && part.image_url) // 过滤出图片 part
    .map(part => {
      // 提取 URL（兼容字符串和对象两种格式）
      return typeof part.image_url === 'string' ? part.image_url : part.image_url.url;
    })
    .filter(Boolean); // 过滤掉空值（undefined、空字符串等）
}

/**
 * 构建 Anthropic 格式的系统提示词（带缓存控制标记）
 *
 * 缓存策略：
 * - Tier 1 (stable): 论文基础信息，会话级缓存
 * - Tier 2 (semi-stable): 线程上下文、段落导览、高优先级上下文，缓存
 * - Tier 3 (dynamic): 网络搜索、RAG 检索、低优先级上下文，不缓存
 *
 * Anthropic prompt caching 要求：
 * - cache_control 标记在想要断开缓存的位置
 * - 如果 Tier 1 变化，Tier 2/Tier 3 都会 miss cache
 * - Tier 1 必须极其稳定（论文基础信息）
 *
 * @param {string|Array<CacheControlBlock>} systemContent - 系统提示词内容
 * @returns {Array<CacheControlBlock>|undefined} 带 cache_control 的 blocks 数组
 */
export function buildAnthropicSystemPrompt(systemContent: string | Array<Record<string, any>> | null | undefined): Array<Record<string, any>> | undefined {
  // 无内容时返回 undefined
  if (!systemContent) return undefined;

  // 简单字符串内容：包装为带 cache_control 的 text block
  if (typeof systemContent === 'string') {
    return [
      {
        type: 'text',
        text: systemContent,
        cache_control: { type: 'ephemeral' },
      },
    ];
  }

  // 已经是 CacheControlBlock[] 格式，直接返回
  // 每个 block 已在 buildSystemPrompt 中根据其 tier 添加了 cache_control
  return systemContent;
}

// ============================================================
// 消息 ID 生成和管理（Phase 1: Inline Threaded Replies）
// ============================================================

/**
 * 生成唯一的消息 ID
 *
 * 设计说明：
 * - 结合时间戳和随机数，确保唯一性
 * - 使用 36 进制编码，缩短字符串长度
 * - 适用于主消息和线程内消息的 ID 生成
 *
 * 为什么使用这种格式？
 * - Date.now().toString(36)：时间戳的 36 进制表示（约 8-9 字符）
 * - Math.random().toString(36).slice(2)：随机数的 36 进制表示（约 10 字符）
 * - 组合后约 18-20 字符，碰撞概率极低
 *
 * @returns {string} 唯一的消息 ID
 *
 * @example
 * const id = generateMessageId(); // => "lw123456789.k2j3h4g5f6d7"
 */
export function generateMessageId() {
  // 时间戳部分（36 进制，确保时序性）
  const timestamp = Date.now().toString(36);

  // 随机数部分（36 进制，防止快速连续创建时碰撞）
  // slice(2) 去掉 "0." 前缀
  const random = Math.random().toString(36).slice(2);

  return timestamp + random;
}

/**
 * 确保所有消息都有 ID（向后兼容旧数据）
 *
 * 用途：
 * - 加载历史消息时，旧消息可能没有 ID 字段
 * - 为线程功能做准备（线程需要消息 ID 进行流式更新）
 *
 * 设计说明：
 * - 递归处理：不仅补填主消息 ID，也补填线程内消息的 ID
 * - 不可变操作：返回新数组，不修改原数组
 * - 使用 spread 操作符而非 structuredClone，保持未变更对象的引用稳定
 *
 * 为什么需要不可变更新？
 * - React 依赖引用变化检测状态更新
 * - 引用不变的组件不会重新渲染（React.memo 优化）
 * - structuredClone 会创建所有对象的新引用，破坏 React.memo
 *
 * @param {Array} messages - 原始消息数组
 * @returns {Array} 确保 ID 存在的新消息数组
 *
 * @example
 * const oldMessages = [{ role: 'user', content: 'Hello' }]; // 无 ID
 * const withIds = ensureMessageIds(oldMessages);
 * // => [{ id: 'lw123...', role: 'user', content: 'Hello', threads: [] }]
 */
export function ensureMessageIds(messages: Array<Record<string, any>>): Array<Record<string, any>> {
  return messages.map((msg) => {
    // 主消息有 ID 则保留，无 ID 则生成新 ID
    const messageId = msg.id || generateMessageId();

    // 处理 threads 数组（递归补填线程内消息的 ID）
    // 如果没有 threads 字段或 threads 为 null/undefined，设为空数组
    if (!msg.threads) {
      return {
        ...msg,
        id: messageId,
        threads: [],
      };
    }

    // 遍历每个线程，检查是否需要补填消息 ID
    let threadsNeedUpdate = false;
    const processedThreads = msg.threads.map((thread: any) => {
      // 检查线程内消息是否需要补填 ID（包括子线程）
      const needsIdUpdate = thread.messages.some((m: any) => {
        if (!m.id) return true;
        // 也检查子线程消息
        if (Array.isArray(m.threads)) {
          return m.threads.some((st: any) => (st.messages || []).some((sm: any) => !sm.id));
        }
        return false;
      });

      if (needsIdUpdate) {
        threadsNeedUpdate = true;
        return {
          ...thread,  // 复制线程对象
          messages: thread.messages.map((m: any) => {
            // 补填主线程消息 ID
            const normalized = m.id ? m : { ...m, id: generateMessageId() };
            // 递归补填子线程消息 ID
            if (Array.isArray(normalized.threads) && normalized.threads.length > 0) {
              const subThreadsNeedUpdate = normalized.threads.some((st: any) => (st.messages || []).some((sm: any) => !sm.id));
              if (subThreadsNeedUpdate) {
                return {
                  ...normalized,
                  threads: normalized.threads.map((st: any) => ({
                    ...st,
                    messages: st.messages.map((sm: any) =>
                      sm.id ? sm : { ...sm, id: generateMessageId() }
                    ),
                  })),
                };
              }
            }
            return normalized;
          }),
        };
      }

      // 无需更新，返回原线程引用
      return thread;
    });

    // 返回新消息对象
    // 如果 threads 数组本身无变更，保留原引用（确保 React.memo 正常工作）
    return {
      ...msg,
      id: messageId,
      threads: threadsNeedUpdate ? processedThreads : msg.threads,
    };
  });
}

// ============================================================
// 聊天搜索工具函数（Phase 6: 搜索支持线程内容）
// ============================================================

/**
 * 转义正则表达式特殊字符
 *
 * @param {string} str - 待转义的字符串
 * @returns {string} 转义后的字符串
 */
export function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * 创建关键词高亮函数
 *
 * 返回一个函数，接收文本并返回高亮后的片段数组。
 * 用于聊天搜索结果中高亮匹配关键词。
 *
 * @param {string} keyword - 搜索关键词
 * @returns {(text: string) => Array<string|JSX.Element>} 高亮函数
 */
export function createKeywordHighlighter(keyword: string): (text: string) => React.ReactNode {
  return function highlightText(text: string): React.ReactNode {
    if (!keyword?.trim() || !text) return text;

    const escaped = escapeRegex(keyword.trim());
    const regex = new RegExp(`(${escaped})`, 'gi');
    const segments = text.split(regex);

    return segments.map((seg, i) => {
      if (seg.toLowerCase() === keyword.trim().toLowerCase()) {
        return <mark key={i} className="chat-search-highlight">{seg}</mark>;
      }
      return seg;
    });
  };
}

/**
 * 检查消息是否匹配搜索关键词（支持线程内消息搜索）
 *
 * Phase 6 增强：除了匹配主消息 content，还会遍历 msg.threads[].messages[].content
 * 进行搜索。这样用户可以在线程内的追问内容中找到相关信息。
 *
 * 设计说明：
 * - 主消息匹配：使用 extractTextFromContent 提取可搜索文本（兼容多模态）
 * - 线程消息匹配：递归遍历每个线程的每条消息
 * - 大小写不敏感：统一转换为小写进行比较
 * - 向后兼容：旧数据无线程时，threads 字段为空数组，不影响搜索
 *
 * 为什么需要这个函数？
 * - ChatPanel.jsx 中的搜索过滤逻辑原只检查主消息
 * - 用户追问的内容可能包含关键信息，应该能被搜索到
 * - 统一搜索入口，便于未来扩展（如搜索附件、搜索标签等）
 *
 * @param {Object} msg - 消息对象，可能包含 threads 数组
 * @param {string} keyword - 搜索关键词（不区分大小写）
 * @returns {{ matches: boolean, matchedThreadIds: Set<string> }} 匹配结果
 *   - matches: 是否匹配（主消息或任一线程消息）
 *   - matchedThreadIds: 匹配的线程 ID 集合（用于自动展开）
 *
 * @example
 * const msg = {
 *   content: "这是主消息",
 *   threads: [
 *     { id: 't1', messages: [{ content: "线程内有关键词" }] }
 *   ]
 * };
 * const result = messageMatchesSearch(msg, "关键词");
 * // => { matches: true, matchedThreadIds: Set(['t1']) }
 */
export function messageMatchesSearch(msg: Record<string, any>, keyword: string | undefined | null): { matches: boolean; matchedThreadIds: Set<string> } {
  // 规范化搜索关键词：去除首尾空格，转为小写
  const normalizedKeyword = keyword?.trim().toLowerCase() || '';
  if (!normalizedKeyword) {
    // 空关键词视为匹配所有消息（返回空的 matchedThreadIds）
    return { matches: true, matchedThreadIds: new Set<string>() };
  }

  const matchedThreadIds = new Set<string>();

  // ===== 主消息匹配检查 =====
  // 提取主消息的纯文本内容进行匹配
  const mainText = extractTextFromContent(msg.content);
  if (mainText.toLowerCase().includes(normalizedKeyword)) {
    // 主消息匹配，返回空的线程 ID 集合
    return { matches: true, matchedThreadIds };
  }

  // ===== 线程消息匹配检查 =====
  // 遍历每个线程，检查线程内消息是否匹配
  const threads = msg.threads || [];
  for (const thread of threads) {
    // 遍历线程内的每条消息
    const threadMessages = thread.messages || [];
    for (const threadMsg of threadMessages) {
      // 提取线程消息的纯文本内容进行匹配
      const threadText = extractTextFromContent(threadMsg.content);
      if (threadText.toLowerCase().includes(normalizedKeyword)) {
        // 找到匹配：记录线程 ID，继续检查其他线程
        matchedThreadIds.add(thread.id);
        break; // 该线程已匹配，无需继续检查该线程的其他消息
      }

      // 检查子线程消息（二级追问）
      if (Array.isArray(threadMsg.threads)) {
        for (const subThread of threadMsg.threads) {
          const subMessages = subThread.messages || [];
          for (const subMsg of subMessages) {
            const subText = extractTextFromContent(subMsg.content);
            if (subText.toLowerCase().includes(normalizedKeyword)) {
              matchedThreadIds.add(thread.id);
              break;
            }
          }
          if (matchedThreadIds.has(thread.id)) break;
        }
      }
      if (matchedThreadIds.has(thread.id)) break;
    }
  }

  // 返回匹配结果：matchedThreadIds 非空表示有线程匹配
  return {
    matches: matchedThreadIds.size > 0,
    matchedThreadIds,
  };
}
