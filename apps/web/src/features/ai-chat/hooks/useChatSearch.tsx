/**
 * AI 聊天搜索 Hook
 *
 * 负责处理聊天消息的搜索和过滤功能，包括：
 * - 搜索关键词状态管理
 * - 消息过滤（支持线程内容搜索）
 * - 关键词高亮
 * - 搜索结果点击跳转
 * - 搜索时自动展开匹配的线程
 *
 * 从 ChatPanel 组件中提取，使代码结构更清晰，职责更单一。
 *
 * @module ai-chat/hooks/useChatSearch
 */

import React, { useCallback, useMemo, useEffect, useRef } from 'react';
import { messageMatchesSearch, createKeywordHighlighter } from '../utils/chatMessageUtils';

/**
 * AI 聊天搜索 Hook
 *
 * @param {object} params - Hook 参数
 * @param {Array} params.messages - 当前消息列表
 * @param {string} params.chatSearchKeyword - 搜索关键词
 * @param {Function} params.onChatSearchClear - 清空搜索回调
 * @param {Function} params.handleExpandThreadById - 展开指定线程的回调
 * @returns {object} 包含搜索相关状态和操作函数的对象
 */
export function useChatSearch({
  messages,
  chatSearchKeyword,
  onChatSearchClear,
  handleExpandThreadById,
}: {
  messages: any[];
  chatSearchKeyword: string;
  onChatSearchClear?: () => void;
  handleExpandThreadById?: (threadId: string) => void;
}) {
  // 搜索关键词直接使用传入的 prop

  // 存储 requestAnimationFrame ID，用于组件卸载时清理
  const rafRefs = useRef<number[]>([]);

  // ========== 自动展开匹配的线程 ==========
  useEffect(() => {
    if (!chatSearchKeyword.trim()) return;

    const threadIdsToExpand = new Set();
    for (const msg of messages) {
      const result = messageMatchesSearch(msg, chatSearchKeyword);
      if (result.matches) {
        result.matchedThreadIds.forEach(id => threadIdsToExpand.add(id));
      }
    }

    if (threadIdsToExpand.size > 0) {
      for (const threadId of threadIdsToExpand as any) {
        handleExpandThreadById?.(threadId as string);
      }
    }
  }, [chatSearchKeyword, messages, handleExpandThreadById]);

  // ========== 消息过滤 ==========
  const filteredMessages = useMemo(() => {
    if (!chatSearchKeyword.trim()) return messages;

    return messages.filter((msg: any) => {
      const result = messageMatchesSearch(msg, chatSearchKeyword);
      return result.matches;
    });
  }, [messages, chatSearchKeyword]);

  // ========== 关键词高亮 ==========
  const highlightChatKeyword = useCallback(createKeywordHighlighter(chatSearchKeyword), [chatSearchKeyword]);

  // ========== 消息索引映射 ==========
  const messageIndexMap = useMemo(() => {
    const map = new Map<any, number>();
    messages.forEach((msg: any, idx: number) => map.set(msg, idx));
    return map;
  }, [messages]);

  // ========== 搜索结果点击跳转 ==========
  const handleSearchResultClick = useCallback((originalIdx: number) => {
    if (onChatSearchClear) onChatSearchClear();

    const rafId1 = requestAnimationFrame(() => {
      const rafId2 = requestAnimationFrame(() => {
        const el = document.getElementById(`chat-msg-${originalIdx}`);
        if (el) {
          el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
      });
      rafRefs.current.push(rafId2);
    });
    rafRefs.current.push(rafId1);
  }, [onChatSearchClear]);

  // 组件卸载时清理
  useEffect(() => {
    return () => {
      rafRefs.current.forEach(id => cancelAnimationFrame(id));
    };
  }, []);

  return {
    filteredMessages,
    highlightChatKeyword,
    messageIndexMap,
    handleSearchResultClick,
  };
}
