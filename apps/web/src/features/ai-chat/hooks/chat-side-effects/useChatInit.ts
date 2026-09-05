/**
 * 聊天初始化 Hook
 *
 * 切换论文（paperId 变化）时：
 * 1. 中断上一个未完成的流式 fetch（abortControllerRef.current.abort()）
 * 2. 清除活跃请求 ID（任何延迟到达的旧流式响应都会被忽略）
 * 3. 并行加载聊天历史 + 用户设置（allSettled，互不影响）+ 探测 agent 能力
 * 4. 桌面端启动时后端可能还在编译，getSettings 带重试（最多 10 次 × 1500ms）
 * 5. P2（web-agent-runtime）：bib 为空但 localStorage 有 thread 映射且后端为
 *    runtime 模式时，从服务端 session 转写水合 UI（对话的真身在 agent.db）
 * 6. P4（web-agent-runtime）：bib 有历史但无映射且后端为 runtime 模式时，
 *    打开面板即预导入旧对话到服务端 session（首条发送前完成迁移）
 *
 * 抽出原因：原 ChatPanel 的 init effect（~115 行）跟消息/UI 编排无关，
 * 独立成 hook 让 ChatPanel 专注于对话编排。
 *
 * Phase 4 注记：当前签名接收一堆 setter，是因为没有 zustand store。
 * 引入 chatStore 后，setter 全部消失，签名收敛为 useChatInit({ paperId, agentType })。
 *
 * @module ai-chat/hooks/chat-side-effects/useChatInit
 */

import { useEffect, useState } from 'react';
import { getMessages } from '../../../../services/chatApi';
import { getSettings } from '../../../../services/settingsApi';
import { ensureThreadStructure } from '../../utils/threadUtils';
import {
  ensureMainThreadSession,
  fetchThreadMessages,
  getStoredThreadId,
  probeAgentMode,
  toImportMessages,
} from '../../api/agentThreadsApi';
import type { AgentType } from '../../agentTypes';

/** useChatInit 参数 */
interface UseChatInitParams {
  paperId: string | number;
  /** 当前面板的 agent 类型 —— runtime thread 映射键的组成部分（reader/screening 不串会话） */
  agentType: AgentType;
  /** 设置消息列表（聊天历史加载完成后调用） */
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  /** 设置用户设置对象 */
  setSettings: React.Dispatch<React.SetStateAction<Record<string, any>>>;
  /** 设置流式状态（paperId 切换中断旧请求时重置） */
  setStreaming: React.Dispatch<React.SetStateAction<boolean>> | ((s: boolean) => void);
  /** 初始化自定义模型配置（从 settings.custom_model_configs 解析） */
  initializeCustomConfigs: (settings: Record<string, unknown>) => void;
  /** 加载模板数据（从 settings 加载） */
  loadTemplates: (settings: any) => void;
  /** 流式请求 AbortController 引用（paperId 变化时中断旧请求） */
  abortControllerRef: React.MutableRefObject<any>;
  /** 活跃请求 ID 引用（paperId 变化时清空） */
  activeRequestIdRef: React.MutableRefObject<any>;
  /** 加载聊天记录失败时调用（已过滤掉 sidecar 未就绪等可忽略错误） */
  onMessagesLoadError?: (errMsg: string) => void;
}

/**
 * 聊天初始化 Hook
 *
 * @returns loading 初始加载状态（true 期间 ChatPanel 渲染 Spin 占位）。
 *   agent 能力探测不在此返回：ChatPanel 为 useChatSender 另行持有同源探测
 *   状态（probeAgentMode 模块级缓存，两处共享一次 fetch）
 */
export function useChatInit({
  paperId,
  agentType,
  setMessages,
  setSettings,
  setStreaming,
  initializeCustomConfigs,
  loadTemplates,
  abortControllerRef,
  activeRequestIdRef,
  onMessagesLoadError,
}: UseChatInitParams): { loading: boolean } {
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    // 切换论文时中断上一个未完成的流式 fetch（防止旧论文的 AI 回复写入新论文）
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
      abortControllerRef.current = null;
      setStreaming(false);
    }
    // 清除活跃请求 ID，确保延迟到达的旧流式响应被忽略
    activeRequestIdRef.current = null;

    const controller = new AbortController();

    // 桌面端启动时后端可能还在编译，自动重试直到成功
    // 最多 10 次 × 1500ms ≈ 15s
    async function getSettingsWithRetry(signal: AbortSignal, maxRetries = 10, delayMs = 1500) {
      for (let i = 0; i < maxRetries; i++) {
        if (signal?.aborted) throw new DOMException('Aborted', 'AbortError');
        try {
          return await getSettings(signal);
        } catch (err) {
          if (signal?.aborted || (err as any)?.name === 'AbortError') throw err;
          if (i < maxRetries - 1) {
            await new Promise(r => setTimeout(r, delayMs));
          } else {
            throw err;
          }
        }
      }
    }

    async function init() {
      try {
        // allSettled：聊天历史和设置互不依赖，一个失败不影响另一个。
        // 模式探测单飞（模块级缓存，多面板共享一次 fetch），自带降级
        // （失败 → 'ephemeral'），不进 allSettled
        const modePromise = probeAgentMode();
        const [msgResult, settingsResult] = await Promise.allSettled([
          getMessages(String(paperId), controller.signal),
          getSettingsWithRetry(controller.signal),
        ]);
        const mode = await modePromise;

        if (controller.signal.aborted) return;

        if (msgResult.status === 'fulfilled') {
          // 套一层 ensureThreadStructure：旧数据可能没有 threads 字段，
          // threadUtils.updateThreadInMessages 等会直接 .map(threads)，undefined 会崩
          const loaded = ensureThreadStructure(msgResult.value.messages || []);
          setMessages(loaded);

          // P2 水合兜底：bib_meta 里该 scope 还是空对话，但存在 thread 映射
          // （对话真身在服务端 session）——拉服务端转写恢复 UI。只恢复主对话
          // 转写；inline 追问线程 P2 不走 session，属已知降级。水合后首轮
          // persistMessages 会把内容写回 bib_meta，此后走正常加载路径
          if (mode === 'runtime' && loaded.length === 0) {
            const storedThreadId = getStoredThreadId(paperId, agentType);
            if (storedThreadId) {
              const threadMessages = await fetchThreadMessages(storedThreadId);
              if (!controller.signal.aborted && threadMessages && threadMessages.length > 0) {
                // 与 bib 加载同一条约定：套 ensureThreadStructure 补 threads 字段，
                // 否则追问线程相关操作 .map(threads) 会踩 undefined
                setMessages(
                  ensureThreadStructure(
                    threadMessages.map((m, i) => ({
                      role: m.role,
                      content: m.text,
                      id: `msg-thread-${storedThreadId.slice(0, 8)}-${i}`,
                    })),
                  ),
                );
              }
            }
          }

          // P4 预导入：bib_meta 里有历史但还没有 thread 映射（旧对话首次
          // 遇到 runtime 后端）——打开面板即把转写迁入服务端 session，
          // 不必等首条发送。与首条发送共用 ensureMainThreadSession 的
          // in-flight 去重（同键并发共享一次请求，不会双开/双写历史）。
          // 尽力而为：失败静默（首条发送会重试并在那里真正报错）
          if (mode === 'runtime' && loaded.length > 0 && !getStoredThreadId(paperId, agentType)) {
            void ensureMainThreadSession(
              paperId,
              agentType,
              undefined,
              toImportMessages(loaded),
            ).catch(() => {});
          }
        } else {
          const err = msgResult.reason;
          if (err?.name !== 'AbortError') {
            const errMsg = err instanceof Error ? err.message : String(err);
            // 启动时 sidecar 可能还没就绪，静默处理连接错误
            if (
              onMessagesLoadError
              && !errMsg.includes('error sending request')
              && !errMsg.includes('Failed to fetch')
            ) {
              onMessagesLoadError(errMsg);
            }
          }
        }

        if (settingsResult.status === 'fulfilled' && settingsResult.value) {
          const settingsData = settingsResult.value;
          setSettings(settingsData?.settings || ({} as Record<string, unknown>));
          initializeCustomConfigs(settingsData?.settings as Record<string, unknown>);
          loadTemplates(settingsData?.settings);
        } else if (settingsResult.status === 'rejected') {
          const err = settingsResult.reason;
          if (err?.name !== 'AbortError') {
            console.warn('加载设置失败:', err instanceof Error ? err.message : String(err));
          }
        }
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
        }
      }
    }
    init();

    return () => {
      controller.abort();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paperId, agentType]);

  return { loading };
}
