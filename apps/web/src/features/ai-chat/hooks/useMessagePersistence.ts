/**
 * useMessagePersistence.js
 * ============================================================
 * 消息持久化 Hook
 *
 * 文件功能：
 * 从 useThreadManager.js 中抽取的持久化逻辑，统一管理消息的保存操作。
 *
 * 核心职责：
 * - 提供立即保存方法（persistMessages）
 * - 提供防抖保存方法（debouncedPersist，2 秒延迟）
 * - 页面卸载前使用 sendBeacon 兜底保存（beforeunload 事件）
 * - payload > 60KB 时自动截断（sendBeacon 64KB 限制）
 *
 * 设计决策：
 * - 使用 ref 存储最新消息列表，避免闭包陷阱
 * - debounce 计时器可清除，支持立即保存绕过防抖
 * - sendBeacon 截断策略保留最近 30 条主消息 + 每线程 3 条消息
 * - 持久化 API 路径：POST /api/papers/{paperId}/chat
 *
 * @module ai-chat/hooks/useMessagePersistence
 */

import { useCallback, useRef, useEffect } from 'react'; // 导入 React 核心钩子

/**
 * 消息持久化 Hook
 *
 * @param {Object} params - Hook 参数
 * @param {React.MutableRefObject<Array>} params.messagesRef - 消息列表的 ref（始终指向最新值）
 * @param {Function} params.persistMessages - 持久化消息到后端的函数
 * @param {string|undefined} params.paperId - 论文 ID（用于构建 API URL）
 * @returns {Object} 持久化控制方法
 * @returns {Function} returns.debouncedPersist - 防抖保存（2 秒延迟）
 * @returns {Function} returns.persistImmediately - 立即保存（绕过防抖）
 */
export function useMessagePersistence({ messagesRef, persistMessages, paperId }: {
  messagesRef: React.MutableRefObject<any[]>;
  persistMessages: (msgs: any[]) => void;
  paperId?: string | number;
}) {
  // ========== 防抖计时器 ==========

  /**
   * 防抖计时器 ref
   *
   * 为什么用 ref 而不是 state？
   * - 计时器 ID 不需要触发重渲染
   * - clearTimeout 操作不应产生副作用
   * - ref 可以在任何地方访问，不受闭包限制
   *
   * @type {React.MutableRefObject<number|null>}
   */
  const persistDebouncedRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ========== 防抖保存 ==========

  /**
   * 调度持久化（防抖版本）
   *
   * 设计要点：
   * - 每次调用清除旧计时器，重置防抖
   * - 2 秒后执行保存，合并频繁操作
   * - 使用 persistImmediately 可绕过防抖
   *
   * @returns {void}
   */
  const debouncedPersist = useCallback(() => {
    // 清除旧计时器（重置防抖）
    if (persistDebouncedRef.current) {
      clearTimeout(persistDebouncedRef.current);
    }
    // 设置新计时器（2 秒后执行）
    persistDebouncedRef.current = setTimeout(() => {
      persistMessages(messagesRef.current);
      persistDebouncedRef.current = null;
    }, 2000);
  }, [persistMessages, messagesRef]);

  /**
   * 立即持久化消息
   *
   * 用途：
   * - AI 回复完成后立即保存（不等防抖）
   * - 用户主动触发保存
   * - 组件卸载前的兜底保存
   *
   * @returns {void}
   */
  const persistImmediately = useCallback(() => {
    // 清除待执行的防抖定时器
    if (persistDebouncedRef.current) {
      clearTimeout(persistDebouncedRef.current);
      persistDebouncedRef.current = null;
    }
    // 立即持久化
    persistMessages(messagesRef.current);
  }, [persistMessages, messagesRef]);

  // ========== 组件卸载清理 ==========

  /**
   * 组件卸载时清除防抖定时器并刷写待保存消息
   *
   * 为什么需要刷写？
   * - 仅清除定时器会丢失最后 2 秒防抖窗口内的消息变更
   * - beforeunload 只在页面关闭时触发，组件卸载（如路由切换）不会触发
   */
  useEffect(() => {
    return () => {
      if (persistDebouncedRef.current) {
        clearTimeout(persistDebouncedRef.current);
        persistDebouncedRef.current = null;
        // 刷写待保存的消息，避免静默丢失
        persistMessages(messagesRef.current);
      }
    };
  }, [persistMessages, messagesRef]);

  // ========== 页面卸载前持久化（beforeunload + sendBeacon）==========

  /**
   * 页面卸载前持久化消息
   *
   * 为什么需要 beforeunload + sendBeacon？
   * - 用户关闭标签页时，正常的 fetch 请求可能被浏览器取消
   * - sendBeacon 是专门为页面卸载场景设计的 API，请求不会被取消
   * - 确保用户追问的内容不会因为直接关闭标签页而丢失
   *
   * 为什么需要渐进截断？
   * - sendBeacon 有 64KB payload 限制（虽然大部分浏览器支持更大）
   * - 长对话历史 + 多线程可能超出限制
   * - 截断策略：保留最近 30 条主消息 + 每线程最近 3 条消息
   */
  useEffect(() => {
    // 无 paperId 时不绑定事件
    if (!paperId) return;

    /**
     * beforeunload 事件处理器
     *
     * 在用户即将离开页面时（刷新、关闭标签页、跳转其他 URL）触发。
     * 使用 sendBeacon 发送消息数据，确保请求不会被取消。
     *
     * @returns {void}
     */
    const handleBeforeUnload = () => {
      // 读取最新的消息列表（使用 ref 避免闭包陷阱）
      const msgs = messagesRef.current;

      // 无消息时不发送请求
      if (!msgs || msgs.length === 0) return;

      // 构建请求 payload
      let payload;
      try {
        payload = JSON.stringify({ messages: msgs });
      } catch (e) {
        console.error('[beforeunload] 序列化消息失败:', e);
        return; // 序列化失败，放弃发送
      }

      // 如果 payload 超过 60KB，进行渐进截断
      // 60KB 是安全阈值，确保不超出 sendBeacon 的 64KB 限制
      if (payload.length > 60000) {
        // 第 1 步：截断至最近 30 条主消息
        let truncated = msgs.slice(-30);

        // 第 2 步：每线程只保留最近 3 条消息
        // 线程消息通常较短，保留 3 条足以维持追问上下文
        truncated = truncated.map((msg: any) => {
          if (!msg.threads || msg.threads.length === 0) return msg;

          return {
            ...msg,
            threads: msg.threads.map((t: any) => ({
              ...t,
              messages: t.messages.slice(-3), // 只保留最近 3 条
            })),
          };
        });

        // 重新序列化截断后的消息
        try {
          payload = JSON.stringify({ messages: truncated });
        } catch (e) {
          console.error('[beforeunload] 截断后序列化失败:', e);
          return; // 序列化失败，放弃发送
        }

        console.warn(`[beforeunload] 消息过大，已截断至最近 30 条主消息 + 每线程 3 条消息`);
      }

      // 使用 sendBeacon 发送请求（不会被页面卸载取消）
      // 注意：sendBeacon 只支持 POST，且无法设置自定义 headers
      // Content-Type 会自动设为 text/plain，后端需要兼容处理
      const apiUrl = `/api/papers/${paperId}/chat`;
      const blob = new Blob([payload], { type: 'application/json' });

      // 发送请求（Fire and Forget，无法获取响应）
      const sent = navigator.sendBeacon(apiUrl, blob);

      if (!sent) {
        console.warn('[beforeunload] sendBeacon 发送失败（可能是浏览器限制或请求被取消）');
        // Fallback: store in sessionStorage for later retry
        const fallbackKey = `pending_messages_${paperId}`;
        try {
          sessionStorage.setItem(fallbackKey, JSON.stringify(payload));
          console.info('[beforeunload] 消息已保存到 sessionStorage，下次访问时将重试发送');
        } catch (err) {
          console.error('[beforeunload] sessionStorage 保存失败:', err);
        }
      }
    };

    // 绑定 beforeunload 事件
    window.addEventListener('beforeunload', handleBeforeUnload);

    // 清理函数：组件卸载时移除事件监听
    return () => {
      window.removeEventListener('beforeunload', handleBeforeUnload);
    };
  }, [paperId, messagesRef]); // 依赖 paperId 和 messagesRef

  // ========== Mount 时重试 pending sessionStorage ==========
  //
  // beforeunload 的 fallback 会把 sendBeacon 失败的消息写入 sessionStorage
  // (key: `pending_messages_${paperId}`), 但旧实现没有读取方 → 静默丢数据。
  // 这里在 mount 时检查并 fetch 重发, 成功后清掉 key。
  useEffect(() => {
    if (paperId === undefined) return;
    const key = `pending_messages_${paperId}`;
    let pending: string | null = null;
    try {
      pending = sessionStorage.getItem(key);
    } catch {
      return; // sessionStorage 不可访问 (隐私模式等) — 静默放弃
    }
    if (!pending) return;

    console.info(`[useMessagePersistence] mount retry: ${key} 有 pending payload, 重新发送`);

    // 直接 fetch (非 sendBeacon), 失败保留 key 等下次 mount 再试。
    fetch(`/api/papers/${paperId}/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: pending,
    })
      .then((res) => {
        if (res.ok) {
          try {
            sessionStorage.removeItem(key);
            console.info(`[useMessagePersistence] retry 成功, 清除 ${key}`);
          } catch {
            // ignore
          }
        } else {
          console.warn(`[useMessagePersistence] retry 失败 (HTTP ${res.status}), 保留 sessionStorage 等下次重试`);
        }
      })
      .catch((err) => {
        console.warn(`[useMessagePersistence] retry 网络错误, 保留 sessionStorage:`, err);
      });
  }, [paperId]);

  // ========== 返回接口 ==========

  return {
    // 防抖保存（2 秒延迟）
    debouncedPersist,

    // 立即保存（绕过防抖）
    persistImmediately,
  };
}

/**
 * 默认导出
 *
 * 为什么使用命名导出 + 默认导出？
 * - 命名导出便于IDE自动导入和重构
 * - 默认导出兼容 import { useMessagePersistence } 和 import useMessagePersistence
 */
export default useMessagePersistence;
