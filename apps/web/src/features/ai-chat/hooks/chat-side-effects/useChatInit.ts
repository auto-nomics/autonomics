/**
 * 聊天初始化 Hook
 *
 * 切换论文（paperId 变化）时：
 * 1. 中断上一个未完成的流式 fetch（abortControllerRef.current.abort()）
 * 2. 清除活跃请求 ID（任何延迟到达的旧流式响应都会被忽略）
 * 3. 并行加载聊天历史 + 用户设置（allSettled，互不影响）
 * 4. 桌面端启动时后端可能还在编译，getSettings 带重试（最多 10 次 × 1500ms）
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

/** useChatInit 参数 */
interface UseChatInitParams {
  paperId: string | number;
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
 * @returns loading 初始加载状态（true 期间 ChatPanel 渲染 Spin 占位）
 */
export function useChatInit({
  paperId,
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
        // allSettled：聊天历史和设置互不依赖，一个失败不影响另一个
        const [msgResult, settingsResult] = await Promise.allSettled([
          getMessages(String(paperId), controller.signal),
          getSettingsWithRetry(controller.signal),
        ]);

        if (controller.signal.aborted) return;

        if (msgResult.status === 'fulfilled') {
          // 套一层 ensureThreadStructure：旧数据可能没有 threads 字段，
          // threadUtils.updateThreadInMessages 等会直接 .map(threads)，undefined 会崩
          setMessages(ensureThreadStructure(msgResult.value.messages || []));
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
  }, [paperId]);

  return { loading };
}
