/**
 * ThreadMessageItem.jsx
 * ============================================================
 * 线程内消息渲染组件
 *
 * 文件功能：
 * 渲染追问线程中的单条消息（用户或 AI）。
 * 简化版 ChatMessageItem，支持基础复制功能和子线程追问。
 *
 * 核心设计决策：
 * - 不传递 threads prop 给 MarkdownRenderer，禁止嵌套线程
 * - 不绑定 mouseup 处理器，不显示"追问"按钮（MVP 限制）
 * - 右键菜单支持"复制"、"收起追问"、"删除追问"操作
 * - 不显示悬浮操作按钮，所有操作通过右键菜单完成
 * - 样式与 ChatMessageItem 保持一致，但更简洁
 *
 * 子线程支持（嵌套追问）：
 * - role === 'assistant' 的消息支持文本选择 + 浮动"追问"按钮
 * - 通过 parentThreadId + wrappedSubThreadAction 回调注入父线程上下文
 * - 子线程内 ThreadMessageItem 不接收 subThreads → 自然终止递归
 * - 流式完成后 200ms 延迟防止过早创建子线程
 *
 * @module ai-chat/components/ThreadMessageItem
 */

import React, { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { Dropdown, Button, App } from 'antd';
import { CopyOutlined, MessageOutlined } from '@ant-design/icons';
import MarkdownRenderer from './MarkdownRenderer';
import { extractTextFromContent } from '../utils/chatMessageUtils';

const FLOATING_BUTTON_CONTAINER_WIDTH = 60;
const STREAMING_END_DELAY = 200;

const ThreadMessageItem = ({
  msg,
  chatSearchKeyword = '',
  onCollapseThread,
  onDeleteThread,
  // 子线程相关 props
  subThreads,
  parentThreadId,
  onSubThreadAction,
  isSubThreadStreamingById,
  isSubThreadCollapsed,
}: {
  msg: any;
  chatSearchKeyword?: string;
  onCollapseThread?: () => void;
  onDeleteThread?: () => void;
  subThreads?: any[];
  parentThreadId?: string;
  onSubThreadAction?: ((type: string, threadId: string, ...args: any[]) => void);
  isSubThreadStreamingById?: ((threadId: string) => boolean);
  isSubThreadCollapsed?: ((threadId: string) => boolean);
}) => {
  const { message: antMessage } = App.useApp();

  const isUser = msg.role === 'user';
  const isAssistant = msg.role === 'assistant';
  const messageText = extractTextFromContent(msg.content);

  // ========== 子线程：文本选择 + 浮动按钮 ==========
  const [selectedText, setSelectedText] = useState('');
  const selectedFingerprintRef = useRef<string | null>(null);
  const [floatingButtonPos, setFloatingButtonPos] = useState<{ top: number; left: number } | null>(null);
  const [streamingEndTime, setStreamingEndTime] = useState<number | null>(null);
  const [streamingReady, setStreamingReady] = useState(false);
  const wasStreamingRef = useRef(false);
  const [isCrossParagraphSelection, setIsCrossParagraphSelection] = useState(false);
  const messageContainerRef = useRef<HTMLDivElement>(null);
  const floatingButtonContainerRef = useRef<HTMLDivElement>(null);

  const isSubThreadStreaming = useMemo(() => {
    if (!isSubThreadStreamingById || !subThreads?.length) return false;
    return subThreads.some(t => isSubThreadStreamingById(t.id));
  }, [isSubThreadStreamingById, subThreads]);

  const canCreateSubThread = useMemo(() => {
    if (!onSubThreadAction || !parentThreadId) return false;
    if (isSubThreadStreaming) return false;
    if (streamingEndTime) return streamingReady;
    return true;
  }, [onSubThreadAction, parentThreadId, isSubThreadStreaming, streamingEndTime, streamingReady]);

  const shouldShowFloatingButtons = useMemo(() => {
    return selectedText.length > 5 && canCreateSubThread;
  }, [selectedText.length, canCreateSubThread]);

  const processTextSelection = useCallback(() => {
    setTimeout(() => {
      const selection = window.getSelection();
      const text = selection?.toString().trim();

      if (text && text.length > 5) {
        setSelectedText(text);

        const range = selection!.getRangeAt(0);
        const rect = range.getBoundingClientRect();
        const containerRect = messageContainerRef.current?.getBoundingClientRect();

        const anchorElement = range.startContainer.parentElement
          ?.closest('[data-source-start]');

        if (!anchorElement) {
          setIsCrossParagraphSelection(false);
          selectedFingerprintRef.current = null;
        } else {
          selectedFingerprintRef.current = anchorElement.getAttribute('data-block-fingerprint');

          const startAnchor = range.startContainer.parentElement?.closest('[data-source-start]');
          const endAnchor = range.endContainer.parentElement?.closest('[data-source-start]');
          setIsCrossParagraphSelection(
            !!(startAnchor && endAnchor && startAnchor !== endAnchor)
          );
        }

        if (containerRect) {
          const buttonContainerWidth = floatingButtonContainerRef.current
            ? floatingButtonContainerRef.current.offsetWidth
            : FLOATING_BUTTON_CONTAINER_WIDTH;

          let left = rect.left - containerRect.left + rect.width / 2 - buttonContainerWidth / 2;
          if (left < 0) left = 0;
          if (left + buttonContainerWidth > containerRect.width) {
            left = containerRect.width - buttonContainerWidth;
          }

          setFloatingButtonPos({
            top: rect.top - containerRect.top - 40,
            left,
          });
        }
      } else {
        setSelectedText('');
        setFloatingButtonPos(null);
        setIsCrossParagraphSelection(false);
        selectedFingerprintRef.current = null;
      }
    }, 10);
  }, []);

  const handleMouseUp = useCallback((e: MouseEvent) => {
    if (!isAssistant) return;
    e.stopPropagation();
    processTextSelection();
  }, [isAssistant, processTextSelection]);

  const handleTouchEnd = useCallback((e: TouchEvent) => {
    if (!isAssistant) return;
    e.stopPropagation();
    setTimeout(() => {
      processTextSelection();
    }, 200);
  }, [isAssistant, processTextSelection]);

  useEffect(() => {
    if (isAssistant && messageContainerRef.current) {
      const container = messageContainerRef.current;
      container.addEventListener('mouseup', handleMouseUp);
      container.addEventListener('touchend', handleTouchEnd);
      return () => {
        container.removeEventListener('mouseup', handleMouseUp);
        container.removeEventListener('touchend', handleTouchEnd);
      };
    }
  }, [isAssistant, handleMouseUp, handleTouchEnd]);

  // Track sub-thread streaming state for delay
  useEffect(() => {
    if (wasStreamingRef.current && !isSubThreadStreaming) {
      setStreamingEndTime(Date.now());
      setStreamingReady(false);
      const timer = setTimeout(() => setStreamingReady(true), STREAMING_END_DELAY);
      wasStreamingRef.current = false;
      return () => clearTimeout(timer);
    }
    if (isSubThreadStreaming) {
      setStreamingEndTime(null);
      setStreamingReady(false);
    }
    wasStreamingRef.current = isSubThreadStreaming;
  }, [isSubThreadStreaming]);

  const handleCreateSubThread = useCallback(() => {
    const fingerprint = selectedFingerprintRef.current;
    if (!fingerprint) {
      const selection = window.getSelection();
      if (!selection || selection.rangeCount === 0) {
        antMessage.warning('请先选中要追问的文字（至少 5 个字符）');
        return;
      }
      const range = selection.getRangeAt(0);
      const anchorElement = range.startContainer.parentElement
        ?.closest('[data-source-start]');
      if (!anchorElement) {
        antMessage.warning('该段落暂不支持追问');
        setFloatingButtonPos(null);
        return;
      }
      const fallbackFingerprint = anchorElement.getAttribute('data-block-fingerprint');
      if (!fallbackFingerprint) {
        antMessage.warning('无法获取段落指纹，请刷新页面后重试');
        setFloatingButtonPos(null);
        return;
      }
      selectedFingerprintRef.current = fallbackFingerprint;
    }

    if (onSubThreadAction && parentThreadId) {
      onSubThreadAction('createSubThread', parentThreadId, {
        fingerprint: selectedFingerprintRef.current,
        anchorText: selectedText,
        messageId: msg.id,
      });
    }

    setFloatingButtonPos(null);
    setIsCrossParagraphSelection(false);
  }, [parentThreadId, selectedText, onSubThreadAction, antMessage]);

  const handleActivateSubThread = useCallback((subThreadId: string) => {
    onSubThreadAction?.('activate', subThreadId);
  }, [onSubThreadAction]);

  // 子线程动作类型映射（InlineThread 使用父级类型名，需映射到子线程类型名）
  const SUB_THREAD_TYPE_MAP: Record<string, string> = {
    'createSubThread': 'createAndActivateSubThread',
    'subThreadSend': 'subThreadSend',
    'toggle': 'subThreadToggle',
    'delete': 'subThreadDelete',
    'messageDelete': 'subThreadMessageDelete',
    'messageEdit': 'subThreadMessageEdit',
    'activate': 'subThreadActivate',
  };

  // 包装子线程回调，注入 parentThreadId 并映射动作类型
  const wrappedSubThreadAction = useCallback((type: string, subThreadId: string, ...args: any[]) => {
    const mappedType = SUB_THREAD_TYPE_MAP[type] || type;
    onSubThreadAction?.(mappedType, parentThreadId!, subThreadId, ...args);
  }, [onSubThreadAction, parentThreadId]);

  // ========== 基础功能 ==========

  const handleCopy = async () => {
    try {
      const text = extractTextFromContent(msg.content);
      if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
        const { writeText } = await import('@tauri-apps/plugin-clipboard-manager');
        await writeText(text);
      } else {
        await navigator.clipboard.writeText(text);
      }
      window.dispatchEvent(new CustomEvent('thread-message-copied', {
        detail: { messageCount: 1 },
      }));
    } catch (err) {
      console.error('复制失败:', err);
    }
  };

  const contextMenuItems = useMemo(() => ({
    items: [
      {
        key: 'copy',
        label: '复制',
        icon: <CopyOutlined />,
        onClick: handleCopy,
      },
      onCollapseThread ? {
        key: 'collapseThread',
        label: '收起追问',
        onClick: onCollapseThread,
      } : null,
      onDeleteThread ? {
        key: 'deleteThread',
        label: '删除追问',
        danger: true,
        onClick: onDeleteThread,
      } : null,
    ].filter(Boolean),
  }), [handleCopy, onCollapseThread, onDeleteThread]);

  const handleContainerClick = useCallback(() => {
    // 文本选择后 mouseup 和 click 事件几乎同时触发，
    // click 不应清除刚完成的选区状态
    const selection = window.getSelection();
    if (selection && selection.toString().trim().length > 0) {
      return;
    }
    setSelectedText('');
    setFloatingButtonPos(null);
    setIsCrossParagraphSelection(false);
  }, []);

  // 稳定化传给 MarkdownRenderer 的 props 引用，
  // 避免每次内部状态变化时因 || [] / || (() => false) 创建新引用导致 memo 失效
  const stableThreads = useMemo(() => subThreads || [], [subThreads]);
  const stableIsStreamingById = useMemo(
    () => isSubThreadStreamingById || (() => false),
    [isSubThreadStreamingById]
  );
  const stableIsCollapsed = useMemo(
    () => isSubThreadCollapsed || (() => false),
    [isSubThreadCollapsed]
  );

  return (
    <Dropdown
      trigger={['contextMenu']}
      menu={contextMenuItems}
    >
      <div
        ref={messageContainerRef}
        style={{
          marginBottom: 8,
          display: 'flex',
          flexDirection: 'column',
          alignItems: isUser ? 'flex-end' : 'flex-start',
          position: isAssistant ? 'relative' : undefined,
        }}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest('.chat-floating-action-btn')) return;
          // 有选中文字时阻止冒泡，防止触发父级 InlineThread 的
          // onActivate → setActiveThread → 整个组件树重渲染 → DOM 重建 → 选区丢失
          const selection = window.getSelection();
          if (selection && selection.toString().trim().length > 0) {
            e.stopPropagation();
            return;
          }
          handleContainerClick();
        }}
      >
        <div style={{
          ...(isUser ? { maxWidth: '85%' } : { width: '100%' }),
          padding: isUser ? '6px 10px' : '4px 0',
          borderRadius: isUser ? '12px 12px 2px 12px' : '0',
          background: isUser
            ? 'var(--accent-content)'
            : 'transparent',
          color: isUser
            ? 'var(--accent-content-text)'
            : 'var(--text-primary, #262626)',
          fontSize: 13,
          lineHeight: isUser ? 1.5 : 1.6,
        }}>
          {isUser ? (
            <div style={{
              fontSize: 13,
              lineHeight: 1.5,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
            }}>
              {messageText}
            </div>
          ) : (
            <MarkdownRenderer
              content={msg.content || ''}
              searchKeyword={chatSearchKeyword}
              threads={stableThreads}
              onThreadAction={wrappedSubThreadAction}
              onSubThreadAction={wrappedSubThreadAction}
              onActivateThread={handleActivateSubThread}
              isThreadStreamingById={stableIsStreamingById}
              isThreadCollapsed={stableIsCollapsed}
              nestingLevel={1}
            />
          )}
        </div>

        {/* 浮动追问按钮（子线程） */}
        {isAssistant && floatingButtonPos && shouldShowFloatingButtons && (
          <div
            ref={floatingButtonContainerRef}
            style={{
              position: 'absolute',
              top: floatingButtonPos.top,
              left: floatingButtonPos.left,
              display: 'flex',
              gap: 4,
              zIndex: 100,
              boxShadow: '0 2px 8px rgba(0,0,0,0.15)',
            }}
          >
            <Button
              type="primary"
              size="small"
              icon={<MessageOutlined />}
              className="chat-floating-action-btn"
              onClick={(e) => {
                e.stopPropagation();
                handleCreateSubThread();
              }}
              style={{ borderRadius: 4 }}
            >
              追问
            </Button>

            {isCrossParagraphSelection && (
              <div style={{
                position: 'absolute',
                top: '100%',
                left: 0,
                marginTop: 4,
                padding: '2px 6px',
                fontSize: 11,
                color: 'var(--text-secondary, #595959)',
                background: 'var(--bg-secondary, #f5f5f5)',
                borderRadius: 4,
                whiteSpace: 'nowrap',
              }}>
                将在线程中引用完整选中内容
              </div>
            )}
          </div>
        )}
      </div>
    </Dropdown>
  );
};

export default React.memo(ThreadMessageItem, (prevProps, nextProps) => {
  if (prevProps.msg.role !== nextProps.msg.role) return false;
  if (prevProps.msg.content !== nextProps.msg.content) return false;
  if (prevProps.chatSearchKeyword !== nextProps.chatSearchKeyword) return false;
  // 子线程相关 props 比较
  if (prevProps.subThreads !== nextProps.subThreads) return false;
  if (prevProps.parentThreadId !== nextProps.parentThreadId) return false;
  if (prevProps.onSubThreadAction !== nextProps.onSubThreadAction) return false;
  if (prevProps.isSubThreadStreamingById !== nextProps.isSubThreadStreamingById) return false;
  if (prevProps.isSubThreadCollapsed !== nextProps.isSubThreadCollapsed) return false;
  return true;
});
