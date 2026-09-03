/**
 * useThreadManager.js
 * ============================================================
 * 内联追问线程状态管理 Hook
 *
 * 文件功能：
 * 从 ChatPanel 中抽取的线程状态管理逻辑，统一管理线程的创建、删除、折叠、发送等操作。
 *
 * 核心职责：
 * - 管理线程的流式状态（streamingThreadId）
 * - 管理线程的折叠状态（collapsedIds）
 * - 处理线程的创建、删除、折叠/展开操作
 * - 处理线程内消息发送（复用 useChatSender 的 sendMessages）
 * - 处理线程持久化（debounce 2s + AI 回复完成时立即保存）
 *
 * 设计决策：
 * - 使用 ref 存储 messages 和 sendMessages，避免闭包陷阱
 * - collapsedIds 使用 Set 数据结构，支持 O(1) 查找和不可变更新
 * - 创建线程时自动展开并聚焦 ThreadInput
 * - 接近线程上限时自动清理 orphaned 线程
 * - 所有更新操作使用不可变模式，确保 React.memo 正常工作
 *
 * @module ai-chat/hooks/useThreadManager
 */

import { useState, useCallback, useRef, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design App 组件，用于获取 modal/message 实例
import { ExclamationCircleOutlined } from '@ant-design/icons'; // 导入图标：感叹号（用于警告对话框）
import type { Paper } from '@/types';
import {
  createThread, // 创建新线程对象
  addMessageToThread, // 向线程添加消息
  updateLastThreadMessageContent, // 更新线程最后一条消息内容（流式更新）
  updateThreadInMessages, // 深层不可变更新 helper
  updateSubThreadInMessages, // 子线程深层不可变更新 helper
  ensureSubThreadStructure, // 确保线程消息有 threads 数组
  MAX_THREAD_MESSAGES, // 每个线程允许的最大消息数量
  MAX_THREADS_PER_MESSAGE, // 每条消息允许的最大线程数量
  MAX_SUBTHREADS_PER_THREAD_MESSAGE, // 每个 assistant 消息允许的最大子线程数量
  MAX_SUBTHREAD_MESSAGES, // 每个子线程允许的最大消息数量
  buildThreadSystemPrompt, // 构建线程追问的系统提示词
  buildSubThreadSystemPrompt, // 构建子线程追问的系统提示词
} from '../utils/threadUtils'; // 导入线程工具函数
import { useMessagePersistence } from './useMessagePersistence'; // Phase 6: 导入持久化 Hook
import type { ChatMessageBase } from '@/types/chat';

/** 线程内的单条消息 */
interface ThreadMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  threads?: ThreadData[];
}

/** 线程数据 */
interface ThreadData {
  id: string;
  anchorFingerprint?: string;
  anchorText: string;
  orphaned?: boolean;
  messages: ThreadMessage[];
}

/** 聊天消息（包含线程） */
interface ChatMessage extends ChatMessageBase {
  id: string;
  content: string;
  threads?: ThreadData[];
}

/** 线程创建数据 */
interface ThreadCreateData {
  fingerprint: string;
  anchorText: string;
}

/** 子线程创建数据 */
interface SubThreadCreateData {
  fingerprint: string;
  anchorText: string;
  messageId?: string;
}

/** useThreadManager 参数 */
interface UseThreadManagerParams {
  messages: ChatMessage[];
  setMessages: React.Dispatch<React.SetStateAction<ChatMessage[]>> | ((msgs: ChatMessage[]) => void);
  streaming: boolean;
  sendMessages: (
    text: string,
    vibeCardRefs: unknown[],
    attachments: unknown[],
    quotedMessage: unknown,
    setQuotedMessage: unknown,
    threadOptions: unknown,
  ) => void;
  persistMessages: ((msgs?: ChatMessage[]) => void) | undefined;
  paperInfo: Paper | null | undefined;
}

/**
 * 内联追问线程状态管理 Hook
 *
 * @param {Object} params - Hook 参数
 * @param {Array} params.messages - 当前消息列表（从 ChatPanel 传入）
 * @param {Function} params.setMessages - 设置消息列表的函数（从 ChatPanel 传入）
 * @param {boolean} params.streaming - 主对话是否正在流式回复中
 * @param {Function} params.sendMessages - 发送消息的函数（从 useChatSender 传入）
 * @param {Function} params.persistMessages - 持久化消息的函数
 * @param {Object} params.paperInfo - 论文基础信息（用于构建线程系统提示词）
 * @returns {Object} 线程管理状态和方法
 */
export function useThreadManager({
  messages, // 当前消息列表
  setMessages, // 设置消息列表的函数
  streaming, // 主对话流式状态
  sendMessages, // 发送消息的函数
  persistMessages, // 持久化消息的函数
  paperInfo, // 论文基础信息
}: UseThreadManagerParams) {
  // 获取 Ant Design App 上下文中的 modal 和 message 实例
  // 必须使用 App.useApp() 而不是静态 Modal.confirm / message.xxx，
  // 因为静态方法在 ConfigProvider 外部渲染，无法继承深色主题
  const { modal, message: antMessage } = App.useApp();

  // ========== Phase 6: 使用持久化 Hook ==========

  /**
   * Phase 6 优化：使用独立的 useMessagePersistence hook
   *
   * 原实现：内联的持久化逻辑（debounce、beforeunload、sendBeacon）
   * Phase 6：抽取为独立 hook，提升代码可维护性
   */
  const messagesRef = useRef(messages);
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  const { debouncedPersist, persistImmediately } = useMessagePersistence({
    messagesRef,
    persistMessages: persistMessages as any,
    paperId: paperInfo?.id,
  });

  // ========== 状态定义 ==========
  // ========== 状态定义 ==========

  /**
   * 当前正在流式回复的线程 ID 集合
   *
   * Phase 6 优化：从单个 string | null 升级为 Set<string>
   *
   * 为什么需要并行流式？
   * - 用户可能在不同线程中同时发起追问
   * - 不应阻塞其他线程的流式回复
   * - 提升用户体验，避免等待
   *
   * 使用 Set 的优势：
   * - O(1) 查找性能：has() 方法检查线程是否在流式
   * - O(1) 添加/删除：add() / delete() 方法
   * - 天然去重：不会有重复的 threadId
   *
   * 不可变更新模式：
   * - 添加：new Set(prev).add(id)
   * - 删除：new Set(prev).delete(id)
   *
   * @type {Set<string>} 正在流式回复的线程 ID 集合
   */
  const [streamingThreadIds, setStreamingThreadIds] = useState(new Set());

  /**
   * 线程独立的 AbortController 映射
   *
   * Phase 6 新增：每个线程使用独立的 AbortController
   *
   * 为什么需要独立的 AbortController？
   * - 支持中止特定线程的流式回复
   * - 多线程并行流式时，可以单独停止某一个线程
   * - 主对话停止时，可以同时停止所有线程
   *
   * 使用 Map 的优势：
   * - O(1) 查找和更新性能
   * - 支持动态添加和删除
   * - 线程 ID 作为键，AbortController 作为值
   *
   * @type {Map<string, AbortController>} 线程 ID 到 AbortController 的映射
   */
  const threadAbortControllersRef = useRef(new Map());

  /**
   * 折叠的线程 ID 集合
   *
   * 为什么用 Set 而不是数组？
   * - O(1) 查找性能：has() 方法检查线程是否折叠
   * - O(1) 添加/删除：add() / delete() 方法
   * - 天然去重：不会有重复的 threadId
   *
   * 为什么不用 Object<string, boolean>？
   * - Set 的语义更清晰（集合而非映射）
   * - Set 的迭代顺序是插入顺序，更可预测
   *
   * 不可变更新模式：
   * - 添加：new Set(prev).add(id)
   * - 删除：new Set(prev).delete(id)
   */
  const [collapsedIds, setCollapsedIds] = useState(new Set());

  // ========== Ref 定义 ==========

  /**
   * sendMessages 函数的 ref
   *
   * 为什么用 ref？
   * - handleThreadSendMessage 需要调用最新的 sendMessages
   * - 避免 sendMessages 变化时导致 handleThreadSendMessage 变化
   * - handleThreadSendMessage 被 useCallback 缓存，减少重渲染
   */
  const sendMessagesRef = useRef(sendMessages);
  useEffect(() => {
    sendMessagesRef.current = sendMessages;
  }, [sendMessages]);

  // ========== 计算属性 ==========

  /**
   * 主对话是否正在流式回复中
   *
   * 从 props 传入，用于区分主对话流式和线程流式
   */
  const isMainStreaming = streaming;

  /**
   * 是否有线程正在流式回复中
   *
   * Phase 6 优化：检查 streamingThreadIds 是否非空
   * - 任意线程在流式时返回 true
   * - 用于 UI 显示流式状态指示器
   */
  const isThreadStreaming = streamingThreadIds.size > 0;

  // ========== 线程创建 ==========

  /**
   * 处理创建新线程
   *
   * 交互流程：
   * 1. 用户在 AI 消息中选中文字，点击"追问"按钮
   * 2. ChatMessageItem 调用此函数，传入 fingerprint 和 anchorText
   * 3. 创建新线程，添加到 messages[msgIdx].threads 数组
   * 4. 自动展开线程（从 collapsedIds 中移除，如果存在）
   * 5. 滚动到 ThreadInput 并聚焦
   *
   * 错误处理：
   * - 接近线程上限（>= 8）时提示用户并自动清理 orphaned 线程
   * - 达到线程上限（>= 10）时拒绝创建并提示
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   * @param {Object} threadData - 线程数据
   * @param {string} threadData.fingerprint - 锚定元素的指纹
   * @param {string} threadData.anchorText - 用户选中的原文
   */
  const handleCreateThread = useCallback((msgIdx: number, threadData: ThreadCreateData) => {
    const { fingerprint, anchorText } = threadData;

    // 防御性检查：消息索引越界
    if (msgIdx < 0 || msgIdx >= messagesRef.current.length) {
      antMessage.error('无法创建追问：消息索引无效');
      return;
    }

    const msg = messagesRef.current[msgIdx];

    // 确保消息有 threads 数组（向后兼容旧消息）
    const currentThreads = msg.threads || [];

    // 检查是否达到线程上限
    if (currentThreads.length >= MAX_THREADS_PER_MESSAGE) {
      antMessage.warning(`已达到每条消息的追问上限（${MAX_THREADS_PER_MESSAGE} 个），请删除一些旧追问后再试`);
      return;
    }

    // 接近上限（>= 8）时自动清理 orphaned 线程
    if (currentThreads.length >= MAX_THREADS_PER_MESSAGE - 2) {
      const orphanedCount = currentThreads.filter((t: any) => t.orphaned).length;
      if (orphanedCount > 0) {
        // 自动清理所有 orphaned 线程
        const cleanedThreads = currentThreads.filter((t: any) => !t.orphaned);
        const newMsg = { ...msg, threads: cleanedThreads };
        const newMessages = messagesRef.current.map((m: any, i: number) =>
          i === msgIdx ? newMsg : m
        );
        setMessages(newMessages);
        antMessage.info(`已自动清理 ${orphanedCount} 个失效追问，释放空间`);
      }
    }

    // 检查该 fingerprint 是否已有线程
    const existingThreads = currentThreads.filter((t: any) => t.anchorFingerprint === fingerprint);
    if (existingThreads.length > 0) {
      // 已有追问，显示提示（但不阻止创建新线程）
      antMessage.info(`💬 该段落已有 ${existingThreads.length} 个追问，你还可以继续追问`);
    }

    // 创建新线程
    const newThread = createThread(fingerprint, anchorText);

    // 更新消息列表（不可变）
    const updatedThreads = [...currentThreads, newThread];
    const newMsg = { ...msg, threads: updatedThreads };
    const newMessages = messagesRef.current.map((m: any, i: number) =>
      i === msgIdx ? newMsg : m
    );
    setMessages(newMessages);

    // 从折叠集合中移除（自动展开）
    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      newSet.delete(newThread.id);
      return newSet;
    });

    // 调度持久化（防抖 2s）
    debouncedPersist();

    // 注意：不再聚焦 ThreadInput（已移除独立输入框）
    // 线程激活由 ChatPanel 的 handleCreateAndActivateThread 处理
  }, [setMessages, debouncedPersist]);

  // ========== 线程删除 ==========

  /**
   * 处理删除线程
   *
   * 交互流程：
   * 1. 用户点击线程的"删除"按钮
   * 2. 显示二次确认对话框（Modal.confirm）
   * 3. 用户确认后从 messages[msgIdx].threads 数组中移除该线程
   * 4. 调度持久化（防抖 2s）
   *
   * 为什么需要二次确认？
   * - 删除操作不可恢复（虽然消息持久化到后端，但前端删除后用户难以找回）
   * - 避免用户误点击删除按钮导致追问丢失
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   * @param {string} threadId - 要删除的线程 ID
   */
  const handleDeleteThread = useCallback((msgIdx: number, threadId: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const thread = currentThreads.find((t: any) => t.id === threadId);
    const messageCount = thread?.messages?.length || 0;

    // 显示二次确认对话框
    modal.confirm({
      title: '确认删除追问？',
      content: `删除后将无法恢复此追问线程（包含 ${messageCount} 条消息）`,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      icon: <ExclamationCircleOutlined />,
      onOk: () => {
        // 用户确认后执行删除
        const updatedThreads = currentThreads.filter((t: any) => t.id !== threadId);
        const newMsg = { ...msg, threads: updatedThreads };
        const newMessages = messagesRef.current.map((m: any, i: number) =>
          i === msgIdx ? newMsg : m
        );
        setMessages(newMessages);

        // 从折叠集合中移除（虽然线程已删除，但清理一下更干净）
        setCollapsedIds(prev => {
          const newSet = new Set(prev);
          newSet.delete(threadId);
          return newSet;
        });

        // 调度持久化（防抖 2s）
        debouncedPersist();
      },
    });
  }, [setMessages, debouncedPersist]);

  // ========== 线程折叠/展开 ==========

  /**
   * 处理切换线程折叠状态
   *
   * 交互流程：
   * 1. 用户点击线程的"收起/展开"按钮
   * 2. 切换线程在 collapsedIds 集合中的状态
   * 3. 不可变更新：new Set(prev).add(id) 或 new Set(prev).delete(id)
   *
   * 为什么用 Set 而不是 thread.collapsed 字段？
   * - 折叠状态是 UI 状态，不应序列化到后端
   * - 用户刷新页面后默认展开所有线程（更好的默认体验）
   * - Set 查找性能 O(1)，比数组查找更快
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引（未使用，但保留接口一致性）
   * @param {string} threadId - 要切换的线程 ID
   */
  const handleToggleThreadCollapse = useCallback((msgIdx: number, threadId: string) => {
    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      if (newSet.has(threadId)) {
        newSet.delete(threadId); // 展开
      } else {
        newSet.add(threadId); // 折叠
      }
      return newSet;
    });
  }, []);

  /**
   * 处理折叠指定消息的所有线程
   *
   * 交互流程：
   * 1. 用户点击"收起所有"按钮
   * 2. 将该消息的所有线程 ID 添加到 collapsedIds 集合
   * 3. 所有线程折叠为一行显示
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   */
  const handleCollapseAll = useCallback((msgIdx: number) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const threadIds = currentThreads.map((t: any) => t.id);

    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      threadIds.forEach((id: string) => newSet.add(id));
      return newSet;
    });
  }, []);

  /**
   * 处理展开指定消息的所有线程
   *
   * 交互流程：
   * 1. 用户点击"展开所有"按钮
   * 2. 从 collapsedIds 集合中移除该消息的所有线程 ID
   * 3. 所有线程展开显示
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   */
  const handleExpandAll = useCallback((msgIdx: number) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const threadIds = currentThreads.map((t: any) => t.id);

    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      threadIds.forEach((id: string) => newSet.delete(id));
      return newSet;
    });
  }, []);

  /**
   * Phase 6: 展开指定线程（用于搜索时自动展开匹配的线程）
   *
   * 使用场景：
   * - 用户搜索聊天内容，搜索命中线程内消息时
   * - 自动从 collapsedIds 中移除该线程 ID，展开线程显示匹配内容
   *
   * @param {string} threadId - 要展开的线程 ID
   */
  const handleExpandThreadById = useCallback((threadId: string) => {
    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      newSet.delete(threadId);
      return newSet;
    });
  }, []);

  // ========== 线程消息发送 ==========

  /**
   * 处理在线程中发送追问消息
   *
   * Phase 6 优化：支持多线程并行流式
   * - 每个线程使用独立的 AbortController
   * - streamingThreadIds 使用 Set 管理多个流式线程
   * - 支持单独中止某个线程（通过 handleAbortThread）
   *
   * 完整流程：
   * 1. 检查线程消息数是否达到上限（MAX_THREAD_MESSAGES）
   * 2. 添加用户消息到线程（带 id）
   * 3. 创建线程专用的 AbortController
   * 4. 添加线程 ID 到 streamingThreadIds
   * 5. 添加空的 assistant 消息到线程（用于流式更新）
   * 6. 调用 sendMessages（传入 abortController）
   * 7. 流式更新使用 updateThreadInMessages（不更新主消息）
   * 8. 流式结束后清理 AbortController 并持久化
   *
   * 为什么需要独立的流式更新路径？
   * - 线程消息不应触发主消息列表的更新
   * - React.memo(ChatMessageItem) 依赖 threads 引用不变
   * - 避免主消息列表不必要的重渲染
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   * @param {string} threadId - 线程 ID
   * @param {string} text - 用户输入的追问文本
   */
  const handleThreadSendMessage = useCallback((msgIdx: number, threadId: string, text: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const thread = currentThreads.find(t => t.id === threadId);
    if (!thread) {
      antMessage.error('追问线程不存在');
      return;
    }

    // 检查消息数上限
    if (thread.messages && thread.messages.length >= MAX_THREAD_MESSAGES) {
      antMessage.warning(`此追问已达到消息上限（${MAX_THREAD_MESSAGES} 条）`);
      return;
    }

    // 生成用户消息 ID
    const userMessageId = Date.now().toString(36) + Math.random().toString(36).slice(2);
    const userMessage = {
      id: userMessageId,
      role: 'user',
      content: text,
    };

    // 添加用户消息到线程
    let updatedThread = addMessageToThread(thread as any, userMessage);

    // 生成 AI 消息 ID（用于流式更新）
    // 注意：不在此时添加空的 assistant 消息到线程。
    // parseStreamResponse 会在流式开始时通过 onStreamUpdate 回调添加 assistant 消息，
    // onStreamUpdate 负责为其分配预生成的 assistantMessageId。
    // 如果在此处预添加，parseStreamResponse 会再添加一个（无 id），导致：
    // 1. 线程中出现两条 assistant 消息（一条空的带 id，一条有内容但无 id）
    // 2. 无 id 的消息导致后端 Pydantic 校验失败（ThreadMessage.id 为必填字段）→ 422 错误
    const assistantMessageId = Date.now().toString(36) + Math.random().toString(36).slice(2);

    // 更新消息列表（不可变，仅包含新添加的用户消息）
    const newMessages = updateThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      () => updatedThread
    );
    setMessages(newMessages as any);

    // ========== Phase 6: 创建线程专用的 AbortController ==========
    // 每个线程有独立的 AbortController，支持单独中止
    const threadAbortController = new AbortController();
    threadAbortControllersRef.current.set(threadId, threadAbortController);

    // ========== Phase 6: 添加线程 ID 到流式集合 ==========
    setStreamingThreadIds(prev => {
      const newSet = new Set(prev);
      newSet.add(threadId);
      return newSet;
    });

    // 构建线程上下文
    const threadContext = {
      anchorText: thread.anchorText,
      assistantReply: msg.content, // AI 原始回复全文
    };

    // 构建线程系统提示词
    const threadSystemPrompt = buildThreadSystemPrompt(threadContext);

    // 调用 sendMessages（传入 abortController）
    sendMessagesRef.current(
      text, // 用户追问文本
      [], // vibeCardRefs（线程不支持）
      [], // attachments（线程不支持）
      null, // quotedMessage（线程不支持）
      null, // setQuotedMessage（线程不支持）
      {
        // Phase 6: 传入线程专用的 AbortController
        abortController: threadAbortController,

        // 线程选项
        threadContext: {
          msgIdx, // 消息索引
          threadId, // 线程 ID
          userMessageId, // 用户消息 ID
          assistantMessageId, // AI 消息 ID
          threadSystemPrompt, // 线程系统提示词
        },
        // 流式更新回调：更新线程内最后一条消息
        // streamParser 对 setMessages 有两种调用方式：
        // 1. 直接传值: setMessages(newArray) — 初始化 assistant 消息（第 68 行）
        // 2. 传 updater 函数: setMessages(prev => newArray) — 流式更新（第 148 行）
        // 这里做适配：将 streamParser 的"消息数组"操作转换为"thread 对象"操作
        onStreamUpdate: (updaterOrValue: any) => {
          setMessages(((prevMessages: any) => {
            return updateThreadInMessages(
              prevMessages,
              msgIdx,
              threadId,
              (thread: any) => {
                if (typeof updaterOrValue === 'function') {
                  // updater 模式：prev 是 thread.messages，返回新 messages 数组
                  return { ...thread, messages: updaterOrValue(thread.messages) };
                } else {
                  // 直接值模式：parseStreamResponse 初始化 assistant 消息时传入
                  // 需要为无 id 的 assistant 消息分配预生成的 assistantMessageId
                  const withIds = updaterOrValue.map((m: any) => {
                    if (m.role === 'assistant' && !m.id) {
                      return { ...m, id: assistantMessageId };
                    }
                    return m;
                  });
                  return { ...thread, messages: [...thread.messages, ...withIds] };
                }
              }
            );
          }) as any);
        },
        // 流式完成回调
        onStreamComplete: () => {
          // ========== Phase 6: 清理线程状态 ==========
          // 从流式集合中移除线程 ID
          setStreamingThreadIds(prev => {
            const newSet = new Set(prev);
            newSet.delete(threadId);
            return newSet;
          });
          // 清理 AbortController
          threadAbortControllersRef.current.delete(threadId);
          // 立即持久化
          persistImmediately();
        },
        // 流式错误回调
        onStreamError: (error: Error) => {
          // ========== Phase 6: 清理线程状态 ==========
          // 从流式集合中移除线程 ID
          setStreamingThreadIds(prev => {
            const newSet = new Set(prev);
            newSet.delete(threadId);
            return newSet;
          });
          // 清理 AbortController
          threadAbortControllersRef.current.delete(threadId);
          // 用户主动中止时显示友好提示，而非错误消息
          const isAbort = error?.name === 'AbortError';
          // 在线程中显示错误消息
          setMessages(((prevMessages: any) => {
            return updateThreadInMessages(
              prevMessages,
              msgIdx,
              threadId,
              (thread: any) => {
                const errMsg = isAbort
                  ? '已停止生成'
                  : `⚠️ 追问失败: ${error.message || error || '未知错误'}`;
                // 检查线程最后一条是否是 assistant 消息（parseStreamResponse 已添加）
                const lastMsg = thread.messages[thread.messages.length - 1];
                if (lastMsg && lastMsg.role === 'assistant') {
                  return updateLastThreadMessageContent(thread, errMsg);
                } else {
                  return addMessageToThread(thread, {
                    id: assistantMessageId,
                    role: 'assistant',
                    content: errMsg,
                  });
                }
              }
            );
          }) as any);
          persistImmediately();
        },
      }
    );
  }, [setMessages, persistImmediately, paperInfo]);

  /**
   * 中止指定线程的流式回复
   *
   * Phase 6 新增：支持单独中止某个线程
   *
   * 使用场景：
   * - 用户点击线程的"停止"按钮
   * - 线程回复过长，用户希望中止
   * - 主对话停止时，需要同时停止所有线程
   *
   * @param {string} threadId - 要中止的线程 ID
   */
  const handleAbortThread = useCallback((threadId: string) => {
    // 获取该线程的 AbortController
    const controller = threadAbortControllersRef.current.get(threadId);
    if (controller) {
      // 中止请求
      controller.abort();
      // 清理 AbortController（onStreamComplete/onStreamError 也会清理，但这里确保同步清理）
      threadAbortControllersRef.current.delete(threadId);
      // 从流式集合中移除线程 ID
      setStreamingThreadIds(prev => {
        const newSet = new Set(prev);
        newSet.delete(threadId);
        return newSet;
      });
    }
  }, []);

  /**
   * 中止所有线程的流式回复
   *
   * Phase 6 新增：支持同时停止所有线程
   *
   * 使用场景：
   * - 用户刷新页面或关闭标签页
   * - 用户切换论文或离开聊天页面
   * - 主对话停止时，可以选择同时停止所有线程
   */
  const handleAbortAllThreads = useCallback(() => {
    // 遍历所有 AbortController 并中止
    threadAbortControllersRef.current.forEach((controller, _threadId) => {
      controller.abort();
    });
    // 清空 AbortController 映射
    threadAbortControllersRef.current.clear();
    // 清空流式线程集合
    setStreamingThreadIds(new Set());
  }, []);

  // ========== 线程内消息删除/编辑 ==========

  /**
   * 处理删除线程内单条消息
   *
   * Phase 6 新增：支持删除线程内的单条消息
   *
   * 交互流程：
   * 1. 用户在线程消息上 hover，显示删除按钮
   * 2. 点击删除按钮，触发此函数
   * 3. 删除用户消息时，如果是最后一条用户消息且有对应 AI 回复，成对删除
   * 4. 立即持久化变更
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   * @param {string} threadId - 线程 ID
   * @param {string} messageId - 要删除的消息 ID
   * @param {boolean} shouldPairDelete - 是否成对删除（用户消息 + AI 回复）
   */
  const handleDeleteThreadMessage = useCallback((msgIdx: number, threadId: string, messageId: string, shouldPairDelete: boolean = false) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const thread = currentThreads.find(t => t.id === threadId);
    if (!thread) return;

    // 构建新的消息数组
    const newMessages = updateThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      (thread: any) => {
        // 过滤掉要删除的消息
        let newMsgs = thread.messages.filter((m: any) => m.id !== messageId);

        // 如果需要成对删除，且删除的是用户消息
        if (shouldPairDelete) {
          const deletedIndex = thread.messages.findIndex((m: any) => m.id === messageId);
          if (deletedIndex !== -1) {
            const deletedMsg = thread.messages[deletedIndex];
            // 如果删除的是用户消息，且下一条是 AI 消息，一起删除
            if (deletedMsg.role === 'user' && deletedIndex + 1 < thread.messages.length) {
              const nextMsg = thread.messages[deletedIndex + 1];
              if (nextMsg.role === 'assistant') {
                // 成对删除：移除下一条 AI 消息
                newMsgs = thread.messages.filter((m: any, idx: number) =>
                  m.id !== messageId && idx !== deletedIndex + 1
                );
              }
            }
          }
        }

        return {
          ...thread,
          messages: newMsgs,
        };
      }
    );

    setMessages(newMessages as any);

    // 立即持久化
    persistMessages?.(newMessages as any);
  }, [persistMessages]);

  /**
   * 处理编辑线程内单条消息
   *
   * Phase 6 新增：支持编辑线程内的用户消息
   *
   * 交互流程：
   * 1. 用户在线程消息上 hover，显示编辑按钮
   * 2. 点击编辑按钮，进入编辑模式
   * 3. 用户修改内容后点击保存，触发此函数
   * 4. 更新消息内容，立即持久化
   *
   * @param {number} msgIdx - 消息在 messages 数组中的索引
   * @param {string} threadId - 线程 ID
   * @param {string} messageId - 要编辑的消息 ID
   * @param {string} newContent - 新的消息内容
   */
  const handleEditThreadMessage = useCallback((msgIdx: number, threadId: string, messageId: string, newContent: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const currentThreads = msg.threads || [];
    const thread = currentThreads.find((t: any) => t.id === threadId);
    if (!thread) return;

    // 只允许编辑用户消息
    const targetMsg = thread.messages.find((m: any) => m.id === messageId);
    if (!targetMsg || targetMsg.role !== 'user') {
      antMessage.warning('只能编辑用户消息');
      return;
    }

    // 构建新的消息数组
    const newMessages = updateThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      (thread: any) => {
        return {
          ...thread,
          messages: thread.messages.map((m: any) =>
            m.id === messageId
              ? { ...m, content: newContent }
              : m
          ),
        };
      }
    );

    setMessages(newMessages as any);

    // 立即持久化
    persistMessages?.(newMessages as any);
  }, [persistMessages]);

  // ========== 子线程创建 ==========

  /**
   * 处理创建子线程（二级追问）
   *
   * 交互流程：
   * 1. 用户在追问线程的 AI 回复中选中文字，点击"追问"按钮
   * 2. 创建新子线程，添加到 assistant 线程消息的 threads 数组
   * 3. 自动展开子线程（从 collapsedIds 中移除）
   *
   * @param {number} msgIdx - 主消息在 messages 数组中的索引
   * @param {string} threadId - 父线程 ID
   * @param {Object} subThreadData - 子线程数据
   * @param {string} subThreadData.fingerprint - 锚定元素的指纹
   * @param {string} subThreadData.anchorText - 用户选中的 AI 回复段落
   */
  const handleCreateSubThread = useCallback((msgIdx: number, threadId: string, subThreadData: SubThreadCreateData) => {
    const { fingerprint, anchorText, messageId } = subThreadData;

    if (msgIdx < 0 || msgIdx >= messagesRef.current.length) {
      antMessage.error('无法创建子追问：消息索引无效');
      return;
    }

    const msg = messagesRef.current[msgIdx];
    const parentThread = (msg.threads || []).find((t: any) => t.id === threadId);
    if (!parentThread) {
      antMessage.error('父线程不存在');
      return;
    }

    // 优先使用 messageId 定位目标 assistant 消息（用户实际选中的那条），
    // 降级到最后一条 assistant 消息（向后兼容旧调用方）
    const targetMsg = messageId
      ? parentThread.messages.find((m: any) => m.id === messageId && m.role === 'assistant')
      : [...parentThread.messages].reverse().find((m: any) => m.role === 'assistant');
    if (!targetMsg) {
      antMessage.error('无法创建子追问：该线程暂无 AI 回复');
      return;
    }

    // 规范化 assistant 消息的 threads 字段
    const normalizedMsg = ensureSubThreadStructure(targetMsg);
    const currentSubThreads = normalizedMsg.threads || [];

    // 检查子线程上限
    if (currentSubThreads.length >= MAX_SUBTHREADS_PER_THREAD_MESSAGE) {
      antMessage.warning(`已达到每条回复的子追问上限（${MAX_SUBTHREADS_PER_THREAD_MESSAGE} 个）`);
      return;
    }

    // 创建新子线程
    const newSubThread = createThread(fingerprint, anchorText);

    // 使用引用相等性定位目标消息，避免 id 缺失时误匹配
    const targetMsgRef = targetMsg;
    const newMessages = updateThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      (thread: any) => {
        return {
          ...thread,
          messages: thread.messages.map((m: any) => {
            if (m !== targetMsgRef) return m;
            const normalized = ensureSubThreadStructure(m as any);
            return {
              ...normalized,
              threads: [...(normalized.threads || []), newSubThread],
            };
          }),
        };
      }
    );
    setMessages(newMessages as any);

    // 自动展开子线程
    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      newSet.delete(newSubThread.id);
      return newSet;
    });

    debouncedPersist();
    return newSubThread.id;
  }, [setMessages, debouncedPersist]);

  // ========== 子线程发送 ==========

  /**
   * 处理在子线程中发送消息（二级追问）
   *
   * 与 handleThreadSendMessage 类似，但使用 updateSubThreadInMessages，
   * 并构建子线程专用的系统提示词（包含父线程对话历史）。
   *
   * @param {number} msgIdx - 主消息索引
   * @param {string} threadId - 父线程 ID
   * @param {string} subThreadId - 子线程 ID
   * @param {string} text - 用户输入的追问文本
   */
  const handleSubThreadSendMessage = useCallback((msgIdx: number, threadId: string, subThreadId: string, text: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const parentThread = (msg.threads || []).find(t => t.id === threadId);
    if (!parentThread) {
      antMessage.error('父线程不存在');
      return;
    }

    // 在父线程消息中找到子线程
    let subThread: any = null;
    let assistantReplyContent = '';
    for (const threadMsg of parentThread.messages) {
      if (Array.isArray(threadMsg.threads)) {
        const found = threadMsg.threads.find((st: any) => st.id === subThreadId);
        if (found) {
          subThread = found;
          assistantReplyContent = threadMsg.content;
          break;
        }
      }
    }
    if (!subThread) {
      antMessage.error('子线程不存在');
      return;
    }

    // 检查消息数上限
    if (subThread.messages && subThread.messages.length >= MAX_SUBTHREAD_MESSAGES) {
      antMessage.warning(`此子追问已达到消息上限（${MAX_SUBTHREAD_MESSAGES} 条）`);
      return;
    }

    // 生成用户消息 ID
    const userMessageId = Date.now().toString(36) + Math.random().toString(36).slice(2);
    const userMessage = {
      id: userMessageId,
      role: 'user',
      content: text,
    };

    // 添加用户消息到子线程
    const updatedSubThread = addMessageToThread(subThread as any, userMessage);

    const assistantMessageId = Date.now().toString(36) + Math.random().toString(36).slice(2);

    // 更新 messages（不可变）
    const newMessages = updateSubThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      subThreadId,
      () => updatedSubThread
    );
    setMessages(newMessages as any);

    // 创建子线程专用的 AbortController
    const threadAbortController = new AbortController();
    threadAbortControllersRef.current.set(subThreadId, threadAbortController);

    // 添加子线程 ID 到流式集合
    setStreamingThreadIds(prev => {
      const newSet = new Set(prev);
      newSet.add(subThreadId);
      return newSet;
    });

    // 构建子线程上下文
    const subThreadContext = {
      anchorText: subThread.anchorText,
      parentAnchorText: parentThread.anchorText,
      assistantReply: assistantReplyContent,
    };

    // 构建子线程系统提示词
    const subThreadSystemPrompt = buildSubThreadSystemPrompt(
      subThreadContext,
      parentThread.messages as any,
      paperInfo as any
    );

    // 调用 sendMessages
    sendMessagesRef.current(
      text,
      [],
      [],
      false,
      null,
      null,
      {
        abortController: threadAbortController,

        threadContext: {
          msgIdx,
          threadId,
          subThreadId,
          userMessageId,
          assistantMessageId,
          threadSystemPrompt: subThreadSystemPrompt,
        },

        onStreamUpdate: (updaterOrValue: any) => {
          setMessages(((prevMessages: any) => {
            return updateSubThreadInMessages(
              prevMessages,
              msgIdx,
              threadId,
              subThreadId,
              (subThread: any) => {
                if (typeof updaterOrValue === 'function') {
                  return { ...subThread, messages: updaterOrValue(subThread.messages) };
                } else {
                  const withIds = updaterOrValue.map((m: any) => {
                    if (m.role === 'assistant' && !m.id) {
                      return { ...m, id: assistantMessageId };
                    }
                    return m;
                  });
                  return { ...subThread, messages: [...subThread.messages, ...withIds] };
                }
              }
            );
          }) as any);
        },

        onStreamComplete: () => {
          setStreamingThreadIds(prev => {
            const newSet = new Set(prev);
            newSet.delete(subThreadId);
            return newSet;
          });
          threadAbortControllersRef.current.delete(subThreadId);
          persistImmediately();
        },

        onStreamError: (error: Error) => {
          setStreamingThreadIds(prev => {
            const newSet = new Set(prev);
            newSet.delete(subThreadId);
            return newSet;
          });
          threadAbortControllersRef.current.delete(subThreadId);
          // 用户主动中止时显示友好提示，而非错误消息
          const isAbort = error?.name === 'AbortError';
          setMessages(((prevMessages: any) => {
            return updateSubThreadInMessages(
              prevMessages,
              msgIdx,
              threadId,
              subThreadId,
              (subThread: any) => {
                const errMsg = isAbort
                  ? '已停止生成'
                  : `⚠️ 追问失败: ${error.message || error || '未知错误'}`;
                const lastMsg = subThread.messages[subThread.messages.length - 1];
                if (lastMsg && lastMsg.role === 'assistant') {
                  return updateLastThreadMessageContent(subThread, errMsg);
                } else {
                  return addMessageToThread(subThread, {
                    id: assistantMessageId,
                    role: 'assistant',
                    content: errMsg,
                  });
                }
              }
            );
          }) as any);
          persistImmediately();
        },
      }
    );
  }, [setMessages, persistImmediately, paperInfo]);

  // ========== 子线程删除 ==========

  /**
   * 处理删除子线程
   *
   * @param {number} msgIdx - 主消息索引
   * @param {string} threadId - 父线程 ID
   * @param {string} subThreadId - 子线程 ID
   */
  const handleDeleteSubThread = useCallback((msgIdx: number, threadId: string, subThreadId: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const parentThread = (msg.threads || []).find(t => t.id === threadId);
    if (!parentThread) return;

    const subThreadMsg = parentThread.messages.find((m: any) =>
      Array.isArray(m.threads) && m.threads.some((st: any) => st.id === subThreadId)
    );
    const messageCount = subThreadMsg
      ?.threads?.find((st: any) => st.id === subThreadId)?.messages?.length || 0;

    modal.confirm({
      title: '确认删除子追问？',
      content: `删除后将无法恢复此子追问线程（包含 ${messageCount} 条消息）`,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      icon: <ExclamationCircleOutlined />,
      onOk: () => {
        const newMessages = updateThreadInMessages(
          messagesRef.current as any[],
          msgIdx,
          threadId,
          (thread: any) => {
            return {
              ...thread,
              messages: thread.messages.map((threadMsg: any) => {
                if (!Array.isArray(threadMsg.threads)) return threadMsg;
                return {
                  ...threadMsg,
                  threads: threadMsg.threads.filter((st: any) => st.id !== subThreadId),
                };
              }),
            };
          }
        );
        setMessages(newMessages as any);

        setCollapsedIds(prev => {
          const newSet = new Set(prev);
          newSet.delete(subThreadId);
          return newSet;
        });

        debouncedPersist();
      },
    });
  }, [setMessages, debouncedPersist]);

  // ========== 子线程折叠/展开 ==========

  /**
   * 处理切换子线程折叠状态
   * 复用 collapsedIds Set（子线程 ID 全局唯一）
   */
  const handleToggleSubThreadCollapse = useCallback((subThreadId: string) => {
    setCollapsedIds(prev => {
      const newSet = new Set(prev);
      if (newSet.has(subThreadId)) {
        newSet.delete(subThreadId);
      } else {
        newSet.add(subThreadId);
      }
      return newSet;
    });
  }, []);

  // ========== 子线程内消息删除/编辑 ==========

  /**
   * 处理删除子线程内单条消息
   *
   * @param {number} msgIdx - 主消息索引
   * @param {string} threadId - 父线程 ID
   * @param {string} subThreadId - 子线程 ID
   * @param {string} messageId - 要删除的消息 ID
   * @param {boolean} shouldPairDelete - 是否成对删除
   */
  const handleDeleteSubThreadMessage = useCallback((msgIdx: number, threadId: string, subThreadId: string, messageId: string, shouldPairDelete: boolean = false) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const parentThread = (msg.threads || []).find(t => t.id === threadId);
    if (!parentThread) return;

    let subThread: any = null;
    for (const threadMsg of parentThread.messages) {
      if (Array.isArray(threadMsg.threads)) {
        const found = threadMsg.threads.find((st: any) => st.id === subThreadId);
        if (found) { subThread = found; break; }
      }
    }
    if (!subThread) return;

    const newMessages = updateSubThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      subThreadId,
      (subThread: any) => {
        let newMsgs = subThread.messages.filter((m: any) => m.id !== messageId);

        if (shouldPairDelete) {
          const deletedIndex = subThread.messages.findIndex((m: any) => m.id === messageId);
          if (deletedIndex !== -1) {
            const deletedMsg = subThread.messages[deletedIndex];
            if (deletedMsg.role === 'user' && deletedIndex + 1 < subThread.messages.length) {
              const nextMsg = subThread.messages[deletedIndex + 1];
              if (nextMsg.role === 'assistant') {
                newMsgs = subThread.messages.filter((m: any, idx: number) =>
                  m.id !== messageId && idx !== deletedIndex + 1
                );
              }
            }
          }
        }

        return { ...subThread, messages: newMsgs };
      }
    );

    setMessages(newMessages as any);
    persistMessages?.(newMessages as any);
  }, [persistMessages]);

  /**
   * 处理编辑子线程内单条消息
   *
   * @param {number} msgIdx - 主消息索引
   * @param {string} threadId - 父线程 ID
   * @param {string} subThreadId - 子线程 ID
   * @param {string} messageId - 要编辑的消息 ID
   * @param {string} newContent - 新的消息内容
   */
  const handleEditSubThreadMessage = useCallback((msgIdx: number, threadId: string, subThreadId: string, messageId: string, newContent: string) => {
    const msg = messagesRef.current[msgIdx];
    if (!msg) return;

    const parentThread = (msg.threads || []).find((t: any) => t.id === threadId);
    if (!parentThread) return;

    let subThread: any = null;
    for (const threadMsg of parentThread.messages) {
      if (Array.isArray(threadMsg.threads)) {
        const found = threadMsg.threads.find((st: any) => st.id === subThreadId);
        if (found) { subThread = found; break; }
      }
    }
    if (!subThread) return;

    const targetMsg = subThread.messages.find((m: any) => m.id === messageId);
    if (!targetMsg || targetMsg.role !== 'user') {
      antMessage.warning('只能编辑用户消息');
      return;
    }

    const newMessages = updateSubThreadInMessages(
      messagesRef.current as any[],
      msgIdx,
      threadId,
      subThreadId,
      (subThread: any) => {
        return {
          ...subThread,
          messages: subThread.messages.map((m: any) =>
            m.id === messageId ? { ...m, content: newContent } : m
          ),
        };
      }
    );

    setMessages(newMessages as any);
    persistMessages?.(newMessages as any);
  }, [persistMessages]);

  /**
   * 中止指定子线程的流式回复
   *
   * @param {string} subThreadId - 子线程 ID
   */
  const handleAbortSubThread = useCallback((subThreadId: string) => {
    const controller = threadAbortControllersRef.current.get(subThreadId);
    if (controller) {
      controller.abort();
      threadAbortControllersRef.current.delete(subThreadId);
      setStreamingThreadIds(prev => {
        const newSet = new Set(prev);
        newSet.delete(subThreadId);
        return newSet;
      });
    }
  }, []);

  // ========== 返回接口 ==========

  return {
    // 状态
    streamingThreadIds, // Phase 6: 正在流式回复的线程 ID 集合（Set）
    isMainStreaming, // 主对话是否正在流式回复中

    // 方法
    handleCreateThread, // 创建新线程
    handleDeleteThread, // 删除线程
    handleToggleThreadCollapse, // 切换线程折叠状态
    handleCollapseAll, // 折叠所有线程
    handleExpandAll, // 展开所有线程
    handleExpandThreadById, // Phase 6: 展开指定线程（用于搜索时自动展开）
    handleThreadSendMessage, // 发送追问消息
    handleAbortThread, // Phase 6: 中止指定线程的流式回复
    handleAbortAllThreads, // Phase 6: 中止所有线程的流式回复
    handleDeleteThreadMessage, // Phase 6: 删除线程内单条消息
    handleEditThreadMessage, // Phase 6: 编辑线程内单条消息

    // 子线程方法
    handleCreateSubThread, // 创建子线程
    handleSubThreadSendMessage, // 子线程发送消息
    handleDeleteSubThread, // 删除子线程
    handleToggleSubThreadCollapse, // 切换子线程折叠状态
    handleDeleteSubThreadMessage, // 删除子线程内单条消息
    handleEditSubThreadMessage, // 编辑子线程内单条消息
    handleAbortSubThread, // 中止子线程流式回复

    // 计算属性 helper（useCallback 保证引用稳定，避免穿透 React.memo 导致级联重渲染）
    isThreadCollapsed: useCallback((threadId: string) => collapsedIds.has(threadId), [collapsedIds]),
    isThreadStreaming: useCallback((threadId: string) => streamingThreadIds.has(threadId), [streamingThreadIds]),
  };
}

export default useThreadManager;
