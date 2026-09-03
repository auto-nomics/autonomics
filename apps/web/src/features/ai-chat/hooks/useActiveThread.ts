/**
 * useActiveThread - 追问线程激活状态管理 Hook
 *
 * 管理追问线程的激活/取消/创建并激活逻辑，
 * 以及计算当前激活线程的信息。
 *
 * 从 ChatPanel 中提取。
 */
import { useState, useCallback, useMemo, useEffect } from 'react';
import type { RefObject } from 'react';
import type { ChatMessageBase } from '@/types/chat';
import { MAX_THREAD_MESSAGES, MAX_SUBTHREAD_MESSAGES } from '../utils/threadUtils';

/** 线程内消息 */
interface ThreadMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  threads?: SubThreadData[];
}

/** 子线程数据 */
interface SubThreadData {
  id: string;
  messages: ThreadMessage[];
}

/** 线程数据 */
interface ThreadData {
  id: string;
  anchorFingerprint?: string;
  anchorText: string;
  messages: ThreadMessage[];
}

/** 聊天消息 */
interface ChatMessage extends ChatMessageBase {
  threads?: ThreadData[];
}

/** 线程创建数据 */
interface ThreadCreateData {
  fingerprint: string;
  anchorText: string;
}

/** 激活的线程信息 */
interface ActiveThreadState {
  threadId: string;
  msgIdx: number;
  subThreadId?: string;
}

/** useActiveThread 参数 */
interface UseActiveThreadParams {
  messages: ChatMessage[];
  /**
   * 读取最新 messages 数组的 getter（Phase 4：替代原 messagesRef.current）
   * - 调用方典型实现：`() => getMessagesForPaper(paperId)` 或 `() => useChatStore.getState().messagesByPaperId[paperId]`
   * - 保留 getter 抽象，不强绑定 store，便于测试与未来迁移
   */
  getMessages: () => ChatMessage[];
  isThreadCollapsed: (threadId: string) => boolean;
  handleToggleThreadCollapse: (msgIdx: number, threadId: string) => void;
  handleCreateThread: (msgIdx: number, data: ThreadCreateData) => void;
  handleCreateSubThread: (msgIdx: number, threadId: string, data: unknown) => string | undefined;
  handleToggleSubThreadCollapse: (subThreadId: string) => void;
  slateInputRef: RefObject<HTMLElement | null>;
}

export function useActiveThread({
  messages,
  getMessages,
  isThreadCollapsed,
  handleToggleThreadCollapse,
  handleCreateThread,
  handleCreateSubThread,
  handleToggleSubThreadCollapse,
  slateInputRef,
}: UseActiveThreadParams) {
  const [activeThread, setActiveThread] = useState<ActiveThreadState | null>(null);

  const handleActivateThread = useCallback((msgIdx: number, threadId: string) => {
    if (isThreadCollapsed(threadId)) {
      handleToggleThreadCollapse(msgIdx, threadId);
    }
    setActiveThread({ threadId, msgIdx });
  }, [isThreadCollapsed, handleToggleThreadCollapse]);

  const handleDeactivateThread = useCallback(() => {
    setActiveThread(null);
  }, []);

  const handleCreateAndActivateThread = useCallback((msgIdx: number, threadData: ThreadCreateData) => {
    handleCreateThread(msgIdx, threadData);
    requestAnimationFrame(() => {
      const msg = getMessages()?.[msgIdx];
      if (msg) {
        const threads = msg.threads || [];
        const matched = [...threads].reverse().find(t => t.anchorFingerprint === threadData.fingerprint);
        if (matched) {
          setActiveThread({ threadId: matched.id, msgIdx });
          slateInputRef.current?.focus?.();
        }
      }
    });
  }, [handleCreateThread, getMessages, slateInputRef]);

  const handleActivateSubThread = useCallback((msgIdx: number, threadId: string, subThreadId: string) => {
    if (isThreadCollapsed(subThreadId)) {
      handleToggleSubThreadCollapse(subThreadId);
    }
    setActiveThread({ threadId, msgIdx, subThreadId });
  }, [isThreadCollapsed, handleToggleSubThreadCollapse]);

  const handleCreateAndActivateSubThread = useCallback((msgIdx: number, threadId: string, subThreadData: unknown) => {
    const newSubThreadId = handleCreateSubThread(msgIdx, threadId, subThreadData);
    if (newSubThreadId) {
      setActiveThread({ threadId, msgIdx, subThreadId: newSubThreadId });
      slateInputRef.current?.focus?.();
    }
  }, [handleCreateSubThread, slateInputRef]);

  const activeThreadInfo = useMemo(() => {
    if (!activeThread) return null;
    const msg = messages[activeThread.msgIdx];
    if (!msg) return null;

    // 子线程分支
    if (activeThread.subThreadId) {
      const parentThread = (msg.threads || []).find(t => t.id === activeThread.threadId);
      if (!parentThread) return null;
      for (const threadMsg of parentThread.messages) {
        if (Array.isArray(threadMsg.threads)) {
          const subThread = threadMsg.threads.find(st => st.id === activeThread.subThreadId);
          if (subThread) {
            return {
              ...subThread,
              isSubThread: true,
              messageCount: subThread.messages?.length || 0,
              isAtLimit: (subThread.messages?.length || 0) >= MAX_SUBTHREAD_MESSAGES,
            };
          }
        }
      }
      return null;
    }

    // 一级线程逻辑
    const thread = (msg.threads || []).find(t => t.id === activeThread.threadId);
    if (!thread) return null;
    return {
      ...thread,
      isSubThread: false,
      messageCount: thread.messages?.length || 0,
      isAtLimit: (thread.messages?.length || 0) >= MAX_THREAD_MESSAGES,
    };
  }, [activeThread, messages]);

  // 如果激活的线程已被删除，自动取消激活
  useEffect(() => {
    if (activeThread && !activeThreadInfo) {
      setActiveThread(null);
    }
  }, [activeThread, activeThreadInfo]);

  return {
    activeThread,
    activeThreadInfo,
    handleActivateThread,
    handleDeactivateThread,
    handleCreateAndActivateThread,
    handleActivateSubThread,
    handleCreateAndActivateSubThread,
  };
}
