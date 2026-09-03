/**
 * 消息级动作 Hook
 *
 * 把 ChatMessageItem dispatcher 期望的回调形状（带 originalIdx/parentThreadId 等
 * 多余参数）适配到 ChatPanel 内部的 setMessages / handleActivateThread 等核心动作。
 *
 * 抽出原因：原 ChatPanel 末尾的 5 个 useCallback（~90 行）都是 adapter，跟消息
 * 编排无关，独立成 hook 让 ChatPanel 专注于对话主流程。
 *
 * @module ai-chat/hooks/chat-side-effects/useMessageActions
 */

import { useCallback } from 'react';

/** useMessageActions 参数 */
interface UseMessageActionsParams {
  /** 设置消息列表（删除消息时调用） */
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  /** 异步持久化消息列表（删除后立即写入后端） */
  persistMessages: (msgs: any[]) => Promise<void>;
  /** 设置引用消息（右键"引用"时调用） */
  setQuotedMessage: (msg: any) => void;
  /** 激活追问线程（msgIdx + threadId） */
  handleActivateThread: (msgIdx: number, threadId: string) => void;
  /** 激活子线程（msgIdx + parentThreadId + subThreadId） */
  handleActivateSubThread: (msgIdx: number, parentThreadId: string, subThreadId: string) => void;
  /** 切换子线程折叠（仅 subThreadId，collapsedIds 全局唯一） */
  handleToggleSubThreadCollapse: (subThreadId: string) => void;
}

/**
 * 消息级动作 Hook
 *
 * @returns 5 个 ready-to-use 回调，可直接传给 ChatMessageItem
 */
export function useMessageActions({
  setMessages,
  persistMessages,
  setQuotedMessage,
  handleActivateThread,
  handleActivateSubThread,
  handleToggleSubThreadCollapse,
}: UseMessageActionsParams) {
  /** 删除指定索引的消息（立即持久化） */
  const handleDeleteMessage = useCallback(async (index: number) => {
    setMessages(prev => {
      const updated = prev.filter((_, i) => i !== index);
      // 异步持久化（不阻塞 UI），失败仅 console
      persistMessages(updated).catch(err => {
        console.error('删除消息后持久化失败:', err);
      });
      return updated;
    });
  }, [setMessages, persistMessages]);

  /** 引用消息（右键"引用"时设置 quotedMessage 状态） */
  const handleQuoteMessage = useCallback((quotedMsg: any) => {
    setQuotedMessage(quotedMsg);
  }, [setQuotedMessage]);

  /**
   * 激活追问线程的工厂：返回接受 threadId 的回调。
   * 用于在 map 渲染中为每条消息创建独立的激活回调。
   */
  const handleActivateThreadForMessage = useCallback((msgIdx: number) => {
    return (threadId: string) => handleActivateThread(msgIdx, threadId);
  }, [handleActivateThread]);

  /**
   * 激活子线程的工厂：返回接受 (originalIdx, parentThreadId, subThreadId) 的回调。
   * originalIdx 在 dispatcher 里冗余传过来（与 msgIdx 重复），这里忽略。
   */
  const handleActivateSubThreadForMessage = useCallback((msgIdx: number) => {
    return (_originalIdx: any, parentThreadId: string, subThreadId: string) =>
      handleActivateSubThread(msgIdx, parentThreadId, subThreadId);
  }, [handleActivateSubThread]);

  /**
   * 子线程折叠切换适配器
   * ChatMessageItem dispatcher 传 (originalIdx, parentThreadId, subThreadId)，
   * 但 handleToggleSubThreadCollapse 只需要 subThreadId。
   */
  const handleSubThreadCollapseAdapter = useCallback(
    (_originalIdx: any, _parentThreadId: any, subThreadId: string) => {
      handleToggleSubThreadCollapse(subThreadId);
    },
    [handleToggleSubThreadCollapse],
  );

  return {
    handleDeleteMessage,
    handleQuoteMessage,
    handleActivateThreadForMessage,
    handleActivateSubThreadForMessage,
    handleSubThreadCollapseAdapter,
  };
}
