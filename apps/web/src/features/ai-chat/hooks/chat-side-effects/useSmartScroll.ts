/**
 * 智能滚动 Hook
 *
 * 实现"用户向上阅读时，流式输出不会强制滚回底部"的体验：
 * - 监听消息列表 scroll 事件，记录用户当前是否停留在底部附近
 * - 流式输出时只有"底部附近"的用户会被自动跟随滚动
 * - 用户向上滚动超过阈值后，流式输出不再打断阅读
 * - 用户重新滚回底部后，恢复自动跟随
 *
 * 抽出原因：原 ChatPanel 的 handleScroll + auto-scroll effect（~70 行）
 * 与消息编排无关，独立成 hook 让 ChatPanel 专注于对话编排。
 *
 * @module ai-chat/hooks/chat-side-effects/useSmartScroll
 */

import { useCallback, useEffect, useRef, type RefObject } from 'react';

/** 距离底部多少像素内视为"底部附近"（自动跟随滚动的阈值） */
const SCROLL_BOTTOM_THRESHOLD = 80;

/** useSmartScroll 参数 */
interface UseSmartScrollParams<TMessageList, TMessagesEnd> {
  /** 消息数组（每次变化都会触发 auto-scroll 判断） */
  messages: unknown[];
  /** 消息列表可滚动容器的 ref */
  messageListRef: RefObject<TMessageList>;
  /** 消息列表底部锚点元素的 ref（用于 scrollIntoView） */
  messagesEndRef: RefObject<TMessagesEnd>;
}

/**
 * 智能滚动 Hook
 *
 * 内部用 ref 而非 state 跟踪 isNearBottom：
 * 滚动事件触发非常频繁，用 state 会引发重渲染浪费；
 * ref 只是"开关"标志，在 effect 中读取即可。
 *
 * 行为：
 * 1. 用户发新消息（lastMsg.role === 'user'）→ 重置 isNearBottom=true 并滚到底部
 * 2. AI 流式更新（lastMsg.role === 'assistant'）→ 仅当 isNearBottom 时跟随
 *
 * @returns isNearBottomRef 当前是否在底部附近（供外部读取，比如流式 chunk 处理），handleScroll 绑定到容器的 onScroll
 */
export function useSmartScroll<TMessageList extends HTMLElement = HTMLElement, TMessagesEnd extends HTMLElement = HTMLElement>(
  { messages, messageListRef, messagesEndRef }: UseSmartScrollParams<TMessageList, TMessagesEnd>,
): {
  isNearBottomRef: React.MutableRefObject<boolean>;
  handleScroll: () => void;
} {
  const isNearBottomRef = useRef(true);

  const handleScroll = useCallback(() => {
    const el = messageListRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    isNearBottomRef.current = distanceFromBottom < SCROLL_BOTTOM_THRESHOLD;
  }, [messageListRef]);

  useEffect(() => {
    const lastMsg = messages[messages.length - 1] as { role?: string } | undefined;
    // 用户刚发消息：重置跟随状态，确保看到 AI 回复的开头
    if (lastMsg?.role === 'user') {
      isNearBottomRef.current = true;
    }
    // 仅当用户在底部附近时才滚动，避免打断阅读
    if (isNearBottomRef.current) {
      messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
    }
  }, [messages, messagesEndRef]);

  return { isNearBottomRef, handleScroll };
}
