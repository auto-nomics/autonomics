/**
 * AI 聊天发送 Hook
 *
 * 封装发送消息到 AI 的核心逻辑，包括：
 * - 系统提示词构建（分层上下文注入）
 * - API 请求构建和发送
 * - SSE 流式响应解析
 * - 错误处理和超时控制
 *
 * @module ai-chat/hooks/useChatSender
 */

import { useCallback, useRef } from 'react';
import type { Paper } from '@/types';
import {
  buildUserMessageContent,
  stripAttachmentsForPersistence,
} from '../utils/chatMessageUtils';
import {
  resolveModelConfig as resolveModelConfigUtil,
  type ResolvedModelConfig,
  type ModelConfig,
} from '../utils/chatUtils';
import { parseAgentStream } from '../utils/streamParser';
import {
  prepareThreadContext as prepareThreadContextUtil,
  DEFAULT_TOKEN_BUDGET,
  type ThreadContext,
  type ThreadOptions,
  type TokenBudgets,
  type PreparedThreadContext,
} from '../utils/threadUtils';
import { compactHistory, stripThreadsForApi } from '../utils/messageCompaction';
import { buildSystemPrompt as buildSystemPromptUtil } from '../utils/systemPromptBuilder';
import { flattenBlocks } from '../utils/promptUtils';
import { getAuthToken } from '../../../services/client';
import {
  ensureMainThreadSession,
  getStoredThreadId,
  threadChatUrl,
  type AgentMode,
} from '../api/agentThreadsApi';
import type { AgentType } from '../agentTypes';
import type { ChatMessageBase } from '@/types/chat';

/** 聊天消息 */
interface ChatMessage extends ChatMessageBase {
  id: string;
  content: string;
}

// ModelConfig 类型已迁移至 ../utils/chatUtils（与 resolveModelConfig 一起）
// ThreadContext / ThreadOptions / TokenBudgets 已迁移至 ../utils/threadUtils（与 prepareThreadContext 一起）

/** 系统上下文注入项 */
interface SystemContextItem {
  text: string;
  priority: 'high' | 'low';
}

/** 文件引用 */
interface FileRef {
  path: string;
}

/** useChatSender 参数 */
interface UseChatSenderParams {
  setMessages: React.Dispatch<React.SetStateAction<ChatMessage[]>> | ((msgs: ChatMessage[]) => void);
  setStreaming: (streaming: boolean) => void;
  persistMessages: () => Promise<void>;
  abortControllerRef: React.MutableRefObject<AbortController | null>;
  activeRequestIdRef?: React.MutableRefObject<string | null>;
  settings: Record<string, unknown>;
  currentModel: ModelConfig | null;
  tokenBudgets: TokenBudgets | Record<string, number> | null;
  paperInfo: Paper | null;
  paper: Paper | null;
  messages: ChatMessage[];
  streaming: boolean;
  additionalSystemContext?: SystemContextItem[] | null;
  threadContextInjection?: boolean;
  onError?: (msg: string) => void;
  customSystemPrompt?: string;
  agentType: AgentType;
  tools?: string[];
  /**
   * 后端 agent 能力（useChatInit 探测、ChatPanel 传入）。
   * 'runtime' 且满足采用条件（已有 thread 映射或全新对话）时主对话走
   * /api/v1/agent/threads/:id/chat，会话转写由服务端持有；
   * 其余情况（含未传）一律走 legacy /api/v1/agent/chat。
   */
  agentMode?: AgentMode;
}

// Constants
// DEFAULT_MAX_TOKENS / DEFAULT_TEMPERATURE 已迁移至 ../utils/chatUtils（与 buildRequestBody 一起）
// DEFAULT_TOKEN_BUDGET / BUDGET_WARNING_RATIO 已迁移至 ../utils/threadUtils（与 prepareThreadContext 一起）
const QUOTED_SNIPPET_LIMIT = 200;
// SSE 流 idle timeout:每次收到 SSE event(text_delta / tool_call_* / ping 等)
// 就 reset 这个 timer。N 秒无任何 event 才 abort。
//
// 为什么从总时长改成 idle:长任务(如批量处理)可能跑几十分钟,
// 总时长 timeout 必中。改 idle 模式后:
// - 后端 service.rs 每 10s 主动发 SSE ping event,正常情况永远不超时
// - 只在真 hang(后端崩溃 / 网络断开 / bug 导致 reader.read() 阻塞)时才 abort
// - 30s 是宽松阈值,远大于 10s heartbeat interval
const STREAM_IDLE_TIMEOUT_MS = 30_000;


/**
 * AI 聊天发送 Hook
 *
 * @param {object} params - Hook 参数
 * @param {Function} params.setMessages - 设置消息列表的函数
 * @param {Function} params.setStreaming - 设置流式状态的函数
 * @param {Function} params.persistMessages - 持久化消息的函数
 * @param {object} params.abortControllerRef - AbortController 引用
 * @param {object} params.activeRequestIdRef - 活跃请求 ID 引用（用于防止旧流式响应覆盖新消息）
 * @param {object} params.settings - 用户设置
 * @param {object} params.currentModel - 当前选中的模型
 * @param {object} params.tokenBudgets - Token 预算配置
 * @param {object} params.paperInfo - 论文基础信息
 * @param {object} params.paper - 论文完整信息
 * @param {Array} params.messages - 当前消息列表
 * @param {boolean} params.streaming - 是否正在流式回复
 * @param {Array|null} params.additionalSystemContext - 额外系统上下文注入
 *   数组形式：Array<{text: string, priority: 'high'|'low'}>
 *   - high 优先级：插入在 Layer 2.5（网络搜索）之后、Layer 3（段落导览）之前
 *   - low 优先级：追加在所有现有 Layer 之后
 *   设计为通用接口，任何模块想注入上下文只需传 prop，不再改 useChatSender
 * @param {boolean} params.threadContextInjection - 线程上下文注入开关（Phase 6 新增）
 *   开启时，主对话会自动包含用户之前的追问及 AI 回复摘要
 * @param {Function} params.onError - 错误回调函数 (errorMsg: string) => void
 *   用于替代之前的错误消息注入模式，接收包含 ⚠️ 前缀的错误消息
 * @returns {object} 包含 sendMessages 函数的对象
 */
export function useChatSender({
  setMessages,
  setStreaming,
  persistMessages,
  abortControllerRef,
  activeRequestIdRef,
  settings,
  currentModel,
  tokenBudgets,
  paperInfo,
  paper,
  messages,
  streaming,
  additionalSystemContext = null,
  threadContextInjection = false,
  onError = () => {},
  customSystemPrompt = '',
  agentType,
  tools = [],
  agentMode = 'ephemeral',
}: UseChatSenderParams) {
  /**
   * 构建分层系统提示词（薄包装函数）
   *
   * 调用 systemPromptBuilder 模块的 buildSystemPrompt 函数。
   */
  const buildSystemPrompt = useCallback(async (params: Record<string, unknown>) => {
    return buildSystemPromptUtil({
      agentType,
      paperInfo: paperInfo as any,
      paperId: paperInfo?.id ?? null,
      paper: paper as any,
      messages: messages as any,
      additionalSystemContext: additionalSystemContext as any,
      threadContextInjection,
      setMessages: setMessages as any,
      customSystemPrompt,
      ...params,
    } as any);
  }, [
    agentType,
    paperInfo,
    paper,
    additionalSystemContext,
    threadContextInjection,
    messages,
    setMessages,
    customSystemPrompt,
  ]);

  /**
   * parseStreamResponse 已提取到独立模块 streamParser.js
   *
   * 为什么提取？
   * - useChatSender.js 文件过大（873 行），需要降低复杂度
   * - parseStreamResponse 是纯函数，独立为模块便于测试和维护
   * - 为 Phase 5 线程发送做准备（线程模式也需要流式解析）
   *
   * 导入路径：import { parseStreamResponse } from '../utils/streamParser'
   */

  // ========== Helper Functions (Issue 5: Extracted from sendMessages) ==========

  /**
   * 解析模型配置（薄包装：把 useChatSender 的 currentModel/settings 传给纯函数）
   */
  const resolveModelConfig = useCallback((): ResolvedModelConfig => {
    return resolveModelConfigUtil(currentModel ?? null, settings);
  }, [currentModel, settings]);

  /**
   * 准备线程上下文（薄包装：把 useChatSender 的状态传给纯函数）
   * 真正实现在 ../utils/threadUtils.prepareThreadContext
   */
  const prepareThreadContext = useCallback((opts: {
    threadOptions: ThreadOptions | null;
    userMessage: string;
  }): PreparedThreadContext | null => {
    return prepareThreadContextUtil({
      threadOptions: opts.threadOptions,
      currentModel: currentModel ?? null,
      settings,
      tokenBudgets,
      messages,
      userMessage: opts.userMessage,
    });
  }, [currentModel, settings, tokenBudgets, messages]);

  /**
   * 发送消息到 AI
   *
   * 核心编排函数，负责：
   * 1. 构建用户消息和消息列表
   * 2. 获取模型配置
   * 3. 构建系统提示词
   * 4. 发送 API 请求
   * 5. 解析流式响应
   * 6. 持久化消息
   *
   * @param {string} text - 用户输入的文本
   * @param {Array} vibeCardRefs - VibeCard 引用列表（可选）
   * @param {Array} attachments - 附件列表（可选）
   * @param {object} quotedMessage - 引用的消息（可选）
   * @param {Function} setQuotedMessage - 设置引用消息的函数
   * @param {object} threadOptions - 线程选项（可选，Phase 5：线程模式）
   * @param {object} threadOptions.threadContext - 线程上下文 { msgIdx, threadId, userMessageId, assistantMessageId, threadSystemPrompt }
   * @param {Function} threadOptions.onStreamUpdate - 线程流式更新回调 (updater: (thread) => thread) => void
   * @param {Function} threadOptions.onStreamComplete - 线程流式完成回调
   * @param {Function} threadOptions.onStreamError - 线程流式错误回调
   * @returns {Promise<void>}
   */
  const sendMessages = useCallback(async (
    text: string,
    vibeCardRefs: unknown[] = [],
    attachments: unknown[] = [],
    quotedMessage: ChatMessage | null = null,
    setQuotedMessage: ((msg: ChatMessage | null) => void) | null = null,
    threadOptions: ThreadOptions | null = null,
    fileRefs: FileRef[] = [],
    // 单次模板 system 段：来自 ContextBridge 模板（如 PDF 选中翻译），
    // 优先级高于 hook 参数 customSystemPrompt（全局自定义 prompt）和默认 persona。
    // 仅对本次请求生效，不持久化到 agentType 级别的 currentCustomPrompt。
    contextSystemPrompt?: string,
  ) => {
    const userMessage = text.trim();
    if (streaming && !threadOptions) return;
    if (!userMessage && (!attachments || attachments.length === 0)) return;

    // 准备线程上下文
    const threadContextData = prepareThreadContext({
      threadOptions,
      userMessage,
    });
    const isThreadMode = threadContextData?.isThreadMode || false;
    const threadContext = threadOptions?.threadContext || null;

    // 引用消息拼接（线程模式不支持引用）
    let finalMessage = userMessage;
    if (quotedMessage && !isThreadMode) {
      const roleLabel = quotedMessage.role === 'assistant' ? 'AI' : '用户';
      const snippet = quotedMessage.content.slice(0, QUOTED_SNIPPET_LIMIT);
      finalMessage = `[引用了${roleLabel}的消息]:\n${snippet.split('\n').map(l => `> ${l}`).join('\n')}\n\n${userMessage}`;
      if (setQuotedMessage) {
        setQuotedMessage(null);
      }
    }

    // 追加文件引用到消息末尾
    if (fileRefs && fileRefs.length > 0) {
      const fileList = fileRefs.map(f => `- ${f.path}`).join('\n');
      finalMessage = `${finalMessage}\n\n[Referenced files]:\n${fileList}`;
    }

    // 构建用户消息内容
    const userContent = buildUserMessageContent(finalMessage, attachments as any[]);

    // 构建新的消息列表（主对话模式）
    let newMessages: any[] = [];
    if (!isThreadMode) {
      const userMsg = {
        role: 'user',
        content: userContent,
        id: `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`,
      };
      newMessages = [...messages, userMsg];
      setMessages(newMessages as any);
    }

    // 设置流式状态
    if (!isThreadMode) {
      setStreaming(true);
    }

    let timeoutId: ReturnType<typeof setTimeout> | null = null;
    let timedOut = false;

    try {
      // 获取模型配置（autonomics 后端持有模型与工具集，前端只需要 model 名
      // 查 token 预算——model_config/api_key 不再上行）
      const { model } = resolveModelConfig();

      let finalApiMessages = [];
      let finalSystemPrompt = '';

      let compressedMessages = [];

      // ===== P2（web-agent-runtime）：主对话运行时会话模式判定 =====
      // 仅主对话（追问线程 P2 仍走 legacy）；采用条件：
      // - agentMode === 'runtime'（ChatPanel 探测后传入，desktop/旧后端自动降级）
      // - 已有 thread 映射，或全新对话（UI 无历史）——bib_meta 时代的旧对话
      //   继续走 ephemeral，避免历史凭空丢失（搬迁是 P4 迁移项）
      // ensureMainThreadSession 失败（后端瞬时不可用）→ 本轮降级 legacy 全量重发
      let runtimeThreadId: string | null = null;
      if (!isThreadMode && agentMode === 'runtime') {
        const scopePaperId = paperInfo?.id ?? null;
        if (getStoredThreadId(scopePaperId, agentType) || messages.length === 0) {
          runtimeThreadId = await ensureMainThreadSession(
            scopePaperId,
            agentType,
            paperInfo?.title || undefined,
          ).catch(() => null);
        }
      }
      const useRuntimeSession = runtimeThreadId !== null;

      if (isThreadMode) {
        finalApiMessages = (threadContextData as any).threadApiMessages.map((m: any) => ({ role: m.role, content: m.content }));
        finalSystemPrompt = (threadContextData as any).threadSystemPrompt;
      } else {
        // 主对话模式：构建系统提示词
        const budgets = tokenBudgets?.budgets || tokenBudgets as any;
        const defaultBudget = tokenBudgets?.default_budget || DEFAULT_TOKEN_BUDGET;
        const tokenBudget = (budgets as Record<string, number>)[model] || defaultBudget;

        // customSystemPrompt 优先级：contextSystemPrompt（单次模板）> customSystemPrompt（hook 参数全局）> 默认 persona
        // - contextSystemPrompt 来自 ContextBridge 模板的 system 段（如 PDF 选中翻译/段落深入解释等场景）
        // - customSystemPrompt 来自用户在 /prompt 弹窗里设置的全局自定义 prompt（按 agentType 持久化）
        const effectiveCustomSystemPrompt = contextSystemPrompt || customSystemPrompt;

        const { systemPrompt, error: promptError } = await buildSystemPrompt({
          userContent,
          newMessages,
          tokenBudget,
          customSystemPrompt: effectiveCustomSystemPrompt,
          // runtime 路径：静态 persona 已在服务端 agent profile，这里构建的
          // 只是每轮动态上下文（论文信息 / 追问摘要 / 额外注入 / 自定义 persona）
          excludePersona: useRuntimeSession,
        });

        finalSystemPrompt = systemPrompt;

        if (!useRuntimeSession && finalSystemPrompt) {
          // buildSystemPrompt 可能返回 string（向后兼容）或 CacheControlBlock[]（Anthropic prompt caching）
          finalApiMessages.push({ role: 'system', content: flattenBlocks(finalSystemPrompt as any) });
        }

        // 渐进式历史压缩（Agent 模式下跳过，由后端处理）。
        // 压缩的是「本轮之前」的 messages（不含刚拼好的用户消息）——
        // 新输入由 body.message 注入，若混进历史种子，模型会看到同一条消息两遍。
        // runtime 会话路径整段跳过：完整转写在服务端 session 里，历史不上行。
        if (useRuntimeSession) {
          compressedMessages = [];
        } else {
          const { messages: compMessages, fallbackReason, errorMessage } = await compactHistory(
            messages as any[],
            tokenBudget,
            paperInfo?.title || '未知论文',
            paperInfo?.id as any,
            true // isAgentMode: 主对话使用 agent 端点，跳过前端压缩
          );
          compressedMessages = compMessages;

          if (fallbackReason) {
            const errorDetail = errorMessage ? `\n错误信息：${errorMessage}` : '';
            onError(`⚠️ 对话历史摘要生成失败，部分早期对话已被截断，当前回复可能缺少部分早期上下文。${errorDetail}\n请检查后端摘要服务是否正常运行。`);
          }

          finalApiMessages.push(...compressedMessages.map((m: any) => ({ role: m.role, content: m.content })));
        }
      }

      // 发起请求
      const abortController = isThreadMode && threadOptions?.abortController
        ? threadOptions.abortController
        : new AbortController();

      if (!isThreadMode) {
        abortControllerRef.current = abortController;
      }

      const requestId = `${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
      if (!isThreadMode && activeRequestIdRef) {
        activeRequestIdRef.current = requestId;
      }

      // idle timer:每个 SSE chunk 到达就 reset,STREAM_IDLE_TIMEOUT_MS 内完全无活动才 abort。
      // parseAgentStream 通过 onActivity 回调 reset。
      const resetIdleTimer = () => {
        if (timeoutId) clearTimeout(timeoutId);
        timeoutId = setTimeout(() => {
          timedOut = true;
          abortController.abort();
        }, STREAM_IDLE_TIMEOUT_MS);
      };
      resetIdleTimer(); // 启动初始 timer

      // ========== Main chat: route through backend agent endpoint ==========
      if (!isThreadMode) {
        // Build messages for the agent backend (exclude system role, only user/assistant)
        // content 直接透传(String 或 parts 数组都接受),不再剥图片:
        // 后端 assembly.rs 用 content_parse::parse_user_content 解析 parts 数组,
        // 把 image_url part 转成 Content::Image block 喂给 LLM。
        // content 一律压平成纯文本：后端 HistoryMessage.content 是 String，
        // 多模态 parts 数组（图片附件）会让整个请求 422
        const agentMessages: Array<{ role: string; content: string }> = compressedMessages
          .filter((m: any) => m.role !== 'system')
          .map((m: any) => ({
            role: m.role,
            content: stripAttachmentsForPersistence(m.content),
          }));

        // runtime 会话路径：ThreadChatRequest 只读 {message, agent_type, context}；
        // 静态 persona 在服务端 profile，历史在服务端 session，均不上行
        const body = useRuntimeSession
          ? {
              message: (stripAttachmentsForPersistence(userContent) as string) || userMessage,
              agent_type: agentType,
              context: finalSystemPrompt || undefined,
            }
          : {
              // 注入的新输入用压平后的完整文本（保留引用消息/文件引用的拼接）；
              // 图片无法走 String 字段，只剩文本部分
              message: (stripAttachmentsForPersistence(userContent) as string) || userMessage,
              // 后端 ChatRequest 只读 {message, agent_type, messages, system_prompt}：
              // 模型由 TUI config 持有、工具集恒为 bib_all_registrations，
              // jayread 时代的 model_config/tools 字段不再发送
              system_prompt: finalSystemPrompt || undefined,
              messages: agentMessages,
              // 显式意图直传后端：用户选了哪个面板就发哪个 agent_type。
              // 后端按此分发 PersonaSpec（见 jayread-agent::agent::persona），
              // 不再让 query classifier 启发式覆盖用户意图。
              agent_type: agentType,
            };
        // autonomics 后端的 agentik 聊天入口（SSE 直连，不经过 services/client，
        // 但鉴权约定一致：设置了 autonomics_token 就带 Bearer 头）。
        // 两条路径的 SSE 事件契约完全一致，解析/超时/中断逻辑共用
        const chatUrl = useRuntimeSession ? threadChatUrl(runtimeThreadId!) : '/api/v1/agent/chat';
        const apiToken = getAuthToken();
        const response = await fetch(chatUrl, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            ...(apiToken ? { Authorization: `Bearer ${apiToken}` } : {}),
          },
          body: JSON.stringify(body),
          signal: abortController.signal,
        });

        if (!response.ok) {
          let errorMsg = `Agent API 请求失败 (${response.status})`;
          try {
            const errData = await response.json();
            errorMsg = errData.error?.message || errData.error || errData.message || errorMsg;
          } catch {}
          // runtime 路径的同智能体并发闸（后端同 type 串行、忙时 409）
          if (response.status === 409) {
            errorMsg = '该助手正在回复上一条消息，请等它完成后再发送';
          }
          onError(errorMsg);
          setStreaming(false);
          if (timeoutId) clearTimeout(timeoutId);
          abortControllerRef.current = null;
          return;
        }

        const isRequestStillActive = () => activeRequestIdRef?.current === requestId;

        const { content: assistantContent, error: streamError } = await parseAgentStream({
          response,
          newMessages: newMessages as any,
          setMessages: setMessages as any,
          isRequestStillActive,
          requestId,
          onActivity: resetIdleTimer,
        });

        if (timeoutId) clearTimeout(timeoutId);
        abortControllerRef.current = null;

        if (streamError) {
          if (streamError === 'CANCELLED') {
            setStreaming(false);
            return;
          }
          console.error('[useChatSender] Stream parsing error:', streamError);
          // 超时场景:parseAgentStream 因 idle timeout 被 abort,没有 SSE error event 到达
          // streamParser 的 updateMessage 没被调用,用户看不到任何反馈
          if (timedOut) {
            setMessages([
              ...messages,
              {
                role: 'assistant',
                content: '⏳ **等待超时**(30秒无响应)\n\n可能原因:\n- LLM 限额超限(429)\n- 网络连接问题\n- 服务端过载\n\n💡 请稍后重试,或切换模型。',
                id: `msg-timeout-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
              },
            ] as any);
          }
          setStreaming(false);
          return;
        }

        const finalMessages = [
          ...newMessages.map((m: any) => ({
            ...m,
            content: stripAttachmentsForPersistence(m.content),
          })),
          {
            role: 'assistant',
            content: assistantContent,
            id: `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`,
          },
        ];
        // 无参调用：让 persistMessages 从 messagesRef.current 读取最新状态
        // 避免用发送时的快照覆盖掉线程等动态数据
        await persistMessages();
        setStreaming(false);
        return;
      }

      // ========== Thread mode: same agent endpoint as main chat ==========
      // 线程追问不再直连提供商代理（那条路要浏览器持有 apiKey，且
      // /api/ai/proxy 在 autonomics 后端不存在）——与主对话共用
      // /api/v1/agent/chat，模型与密钥都留在服务端。
      const threadEntries = finalApiMessages.filter((m: any) => m.role !== 'system');
      // threadApiMessages 的最后一条就是本轮新输入：拆出来走 message 注入，
      // 其余作为历史种子（与主对话同样的去重约定）
      const lastThreadEntry = threadEntries[threadEntries.length - 1];
      const threadHistory = threadEntries.slice(0, -1).map((m: any) => ({
        role: m.role,
        content: stripAttachmentsForPersistence(m.content),
      }));
      const threadInput = lastThreadEntry
        ? stripAttachmentsForPersistence(lastThreadEntry.content)
        : userMessage;

      const threadBody = {
        message: threadInput || userMessage,
        system_prompt: finalSystemPrompt || undefined,
        messages: threadHistory,
        agent_type: agentType,
      };
      const threadToken = getAuthToken();
      const response = await fetch('/api/v1/agent/chat', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(threadToken ? { Authorization: `Bearer ${threadToken}` } : {}),
        },
        body: JSON.stringify(threadBody),
        signal: abortController.signal,
      });

      if (!response.ok) {
        let errorMsg = `Agent API 请求失败 (${response.status})`;
        try {
          const errData = await response.json();
          errorMsg = errData.error?.message || errData.error || errData.message || errorMsg;
        } catch {}

        if (threadOptions?.onStreamError) {
          threadOptions.onStreamError(new Error(errorMsg));
          return;
        }
        onError(`⚠️ ${errorMsg}`);
        setStreaming(false);
        if (timeoutId) clearTimeout(timeoutId);
        return;
      }

      const isRequestStillActive = () => true;

      const { content: assistantContent, error: streamError } = await parseAgentStream({
        response,
        newMessages: [],
        setMessages: (threadOptions?.onStreamUpdate || setMessages) as any,
        isRequestStillActive,
        onActivity: resetIdleTimer,
      });

      if (timeoutId) clearTimeout(timeoutId);

      if (streamError) {
        if (streamError === 'CANCELLED') {
          setStreaming(false);
          return;
        }

        if (threadOptions?.onStreamError) {
          threadOptions.onStreamError(new Error(streamError));
          return;
        }

        setStreaming(false);
        return;
      }

      if (threadOptions?.onStreamComplete) {
        threadOptions.onStreamComplete();
      }

    } catch (err: any) {
      if (isThreadMode && threadOptions?.onStreamError) {
        if (err.name === 'AbortError') {
          if (timedOut) {
            threadOptions.onStreamError(new Error('AI 回复超时（120 秒），请检查网络或稍后重试。'));
          }
          return;
        }
        threadOptions.onStreamError(err instanceof Error ? err : new Error(String(err)));
        return;
      }

      if (err.name === 'AbortError') {
        if (timedOut) {
          onError('AI 回复超时（120 秒），请检查网络或稍后重试。');
        }
        return;
      }
      onError(`AI 回复失败: ${err.message || err}`);
    } finally {
      if (timeoutId) clearTimeout(timeoutId);
      if (!isThreadMode) {
        abortControllerRef.current = null;
      }
      if (!isThreadMode) {
        setStreaming(false);
      }
    }
  }, [
    streaming,
    messages,
    currentModel,
    settings,
    tokenBudgets,
    buildSystemPrompt,
    resolveModelConfig,
    prepareThreadContext,
    setMessages,
    setStreaming,
    persistMessages,
    abortControllerRef,
    activeRequestIdRef,
    paperInfo,
    onError,
    agentMode,
    agentType,
  ]);

  return { sendMessages };
}
