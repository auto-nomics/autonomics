/**
 * InlineThread.jsx
 * ============================================================
 * 内联追问线程容器组件
 *
 * 文件功能：
 * 渲染一个可折叠的追问线程，显示在锚定文本下方。
 * 线程包含用户和 AI 的对话历史，支持展开/折叠、删除和继续追问。
 *
 * 核心设计决策：
 * - 使用 React.memo + 自定义比较函数优化性能，只在关键变化时重渲染
 * - grid-template-rows 动画实现平滑的展开/折叠效果（替代 max-height）
 * - 失效线程（orphaned）使用虚线边框和警告图标区分
 * - 搜索激活时显示提示横幅，告知用户搜索仅匹配主消息内容
 * - 内置轻量级 ErrorBoundary，渲染失败时降级显示
 *
 * TODO: Phase 6 - 同一块级元素 3+ 线程时合并显示
 *   当前实现：每个线程独立渲染，同一段落的多个线程垂直堆叠
 *   Phase 6 优化：当同一块级元素有 3 个及以上线程时，合并为一个组件：
 *   - 默认折叠：显示 "💬 N 条追问" 折叠态
 *   - 点击展开：显示所有线程的列表（标签页或垂直列表）
 *   - 减少 UI 噪音：避免段落下方出现过多线程
 *
 * @module ai-chat/components/InlineThread
 */

import React, { Component, useState, useCallback, useMemo, useRef, useLayoutEffect } from 'react';
import { App } from 'antd';
import { ExclamationCircleOutlined, DeleteOutlined, MessageOutlined } from '@ant-design/icons';
import ThreadMessageItem from './ThreadMessageItem';
import { MAX_THREAD_MESSAGES } from '../utils/threadUtils';

/**
 * Phase 6: 线程组合并入口组件
 * ============================================================
 * 当同一块级元素有 3 个及以上追问线程时，使用此组件合并显示
 *
 * 设计目的：
 * - 减少 UI 噪音：避免段落下方出现过多线程占用屏幕空间
 * - 优化用户交互：默认折叠，用户按需展开查看所有追问
 *
 * 组件结构：
 * - 折叠态：显示"💬 查看 N 个追问"按钮
 * - 展开态：平铺显示所有 InlineThread 组件
 * - 使用 grid-template-rows 实现平滑展开/折叠动画
 *
 * @param {Object} props - 组件属性
 * @param {Array} props.threads - 线程数组（同属一个指纹）
 * @param {Function} props.onToggle - 切换线程折叠状态的回调 (threadId) => void
 * @param {Function} props.onDelete - 删除线程的回调 (threadId) => void
 * @param {boolean} props.isStreaming - 是否正在流式回复中
 * @param {string} props.chatSearchKeyword - 聊天搜索关键词
 * @param {Function} props.isThreadStreaming - 判断指定线程是否在流式回复中 (threadId) => boolean（Phase 6：并行流式支持）
 * @param {Function} props.onActivate - 追问线程激活回调 (threadId) => void
 */
const ThreadGroupEntry = ({
  threads,
  onToggle,
  onDelete,
  isStreaming,
  chatSearchKeyword = '',
  isThreadStreaming = () => false, // Phase 6: 默认返回 false（向后兼容）
  onActivate = null, // 追问线程激活回调 (threadId) => void
  // 子线程相关 props
  onSubThreadAction = null,
  isSubThreadStreamingById = null,
  isSubThreadCollapsed = null,
  nestingLevel = 0,
}: {
  threads: any[];
  onToggle?: (threadId: string) => void;
  onDelete?: (threadId: string) => void;
  isStreaming?: boolean;
  chatSearchKeyword?: string;
  isThreadStreaming?: (threadId: string) => boolean;
  onActivate?: ((threadId: string) => void) | null;
  onSubThreadAction?: ((...args: any[]) => void) | null;
  isSubThreadStreamingById?: ((threadId: string) => boolean) | null;
  isSubThreadCollapsed?: ((threadId: string) => boolean) | null;
  nestingLevel?: number;
}) => {
  /**
   * 展开状态管理
   * - false: 折叠态，显示"查看 N 个追问"按钮
   * - true: 展开态，显示所有线程
   */
  const [expanded, setExpanded] = useState(false);

  /**
   * 自适应缩进补偿
   *
   * 追问组合并入口可能被渲染在列表（ul/ol）或引用（blockquote）内部，
   * 继承了这些元素的 padding-left 缩进（列表 24px/层，引用 12px）。
   * 通过测量实际偏移量并应用负 margin，使追问区域始终与消息内容区域左对齐。
   */
  const groupRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = groupRef.current;
    if (!el) return;
    const markdownRoot = el.closest('.markdown-content');
    if (!markdownRoot) return;
    const rootLeft = markdownRoot.getBoundingClientRect().left;
    const elLeft = el.getBoundingClientRect().left;
    const offset = elLeft - rootLeft;
    if (offset > 2) {
      el.style.marginLeft = `-${offset}px`;
      el.style.width = `calc(100% + ${offset}px)`;
    }
  }, []);

  /**
   * 处理展开/折叠切换
   * 使用 grid-template-rows 动画实现平滑过渡
   */
  const handleToggle = useCallback(() => {
    setExpanded((prev) => !prev);
  }, []);

  /**
   * 计算线程总数
   * 用于显示"N 个追问"文本
   */
  const threadCount = threads?.length || 0;

  /**
   * 计算总消息数
   * 统计所有线程中的消息总数，用于显示更多信息
   */
  const totalMessages = useMemo(() => {
    return threads.reduce((sum: number, thread: any) => sum + (thread.messages?.length || 0), 0);
  }, [threads]);

  /**
   * 渲染折叠态入口按钮
   * 显示"💬 查看 N 个追问"文本，点击展开
   */
  const renderCollapsedEntry = () => {
    return (
      <div
        onClick={handleToggle}
        style={{
          marginLeft: 0,
          marginBottom: 8,
          padding: '6px 10px',
          background: 'var(--bg-secondary, #f5f5f5)',
          border: '1px solid var(--border-color-light, #e8e8e8)',
          borderRadius: 6,
          cursor: 'pointer',
          display: 'inline-flex',
          alignItems: 'center',
          gap: 6,
          fontSize: 12,
          color: 'var(--text-secondary, #595959)',
          transition: 'all 0.2s',
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.background = 'var(--border-color-light, #e8e8e8)';
          e.currentTarget.style.borderColor = 'var(--border-color-secondary, #d9d9d9)';
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.background = 'var(--bg-secondary, #f5f5f5)';
          e.currentTarget.style.borderColor = 'var(--border-color-light, #e8e8e8)';
        }}
      >
        <MessageOutlined />
        <span>
          查看 {threadCount} 个追问（共 {totalMessages} 条消息）
        </span>
        <span style={{ fontSize: 10, color: 'var(--text-tertiary, #8c8c8c)' }}>
          ▶
        </span>
      </div>
    );
  };

  /**
   * 渲染展开态内容
   * - 显示收起按钮
   * - 平铺显示所有线程
   * - 使用 grid-template-rows 实现平滑动画
   */
  const renderExpandedContent = () => {
    return (
      <div style={{ marginBottom: 8 }}>
        {/* 收起按钮 */}
        <div
          onClick={handleToggle}
          style={{
            marginLeft: 0,
            padding: '4px 8px',
            marginBottom: 4,
            cursor: 'pointer',
            fontSize: 11,
            color: 'var(--text-tertiary, #8c8c8c)',
            display: 'inline-flex',
            alignItems: 'center',
            gap: 4,
            transition: 'color 0.2s',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.color = 'var(--text-secondary, #595959)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.color = 'var(--text-tertiary, #8c8c8c)';
          }}
        >
          <span>▼</span>
          <span>收起追问</span>
        </div>

        {/* 线程列表容器：使用 grid 实现展开动画 */}
        <div
          style={{
            display: 'grid',
            gridTemplateRows: '1fr',
            transition: 'grid-template-rows 0.3s ease-out',
          }}
        >
          <div style={{ overflow: 'hidden' }}>
            {/* 平铺显示所有线程 */}
            {threads.map((thread: any) => (
              <InlineThread
                key={thread.id}
                thread={thread}
                collapsed={false}
                onToggle={onToggle}
                onDelete={onDelete}
                isStreaming={isThreadStreaming(thread.id)} // Phase 6: 使用函数检查每个线程的流式状态
                chatSearchKeyword={chatSearchKeyword}
                onActivate={onActivate}
                nestingLevel={nestingLevel}
                onSubThreadAction={onSubThreadAction}
                isSubThreadStreamingById={isSubThreadStreamingById}
                isSubThreadCollapsed={isSubThreadCollapsed}
              />
            ))}
          </div>
        </div>
      </div>
    );
  };

  return (
    <div ref={groupRef} className="thread-group-entry">
      {/* 根据展开状态渲染不同内容 */}
      {expanded ? renderExpandedContent() : renderCollapsedEntry()}
    </div>
  );
};

/**
 * 使用 React.memo 优化 ThreadGroupEntry 性能
 * 只在线程数量或线程引用变化时重渲染
 *
 * Phase 6 注意：由于 isThreadStreaming 是函数，memo 无法直接比较
 * 因此只在必要时重渲染（线程数量、搜索关键词变化）
 */
const MemoizedThreadGroupEntry = React.memo(ThreadGroupEntry, (prevProps: any, nextProps: any) => {
  const prevThreads = prevProps.threads;
  const nextThreads = nextProps.threads;

  // 线程数量变化时重渲染
  if (prevThreads.length !== nextThreads.length) return false;

  // 流式状态变化时重渲染
  if (prevProps.isStreaming !== nextProps.isStreaming) return false;

  // Phase 6: isThreadStreaming 是函数，无法直接比较
  // 因此假设流式状态总是变化的，确保 UI 及时更新
  // 优化：可以在调用方传入一个 streamingVersion 来避免过度重渲染

  // 搜索关键词变化时重渲染
  if (prevProps.chatSearchKeyword !== nextProps.chatSearchKeyword) return false;

  // 子线程相关 props 变化时重渲染
  if (prevProps.nestingLevel !== nextProps.nestingLevel) return false;
  if (prevProps.onSubThreadAction !== nextProps.onSubThreadAction) return false;
  if (prevProps.isSubThreadStreamingById !== nextProps.isSubThreadStreamingById) return false;
  if (prevProps.isSubThreadCollapsed !== nextProps.isSubThreadCollapsed) return false;

  return true;
});

/**
 * 轻量级 ErrorBoundary 类组件
 *
 * 为什么在文件内定义而非外部文件？
 * - 仅用于 InlineThread 的错误捕获，职责单一
 * - 避免创建额外的小文件，减少项目文件数
 * - 错误时显示"线程加载失败"，不影响主消息渲染
 */
class ThreadErrorBoundary extends Component<{ children?: React.ReactNode }, { hasError: boolean }> {
  /**
   * 构造函数
   * @param {Object} props - 组件属性
   */
  constructor(props: { children?: React.ReactNode }) {
    super(props);
    // 初始化错误状态，null 表示无错误
    this.state = { hasError: false };
  }

  /**
   * 静态方法：捕获子组件抛出的错误
   * React 会在子组件渲染出错时调用此方法
   * @param {Error} error - 捕获的错误对象
   * @returns {Object} 新的状态对象
   */
  static getDerivedStateFromError(error: Error) {
    // 更新状态以显示降级 UI
    return { hasError: true };
  }

  /**
   * 渲染方法
   * @returns {JSX.Element} 错误降级 UI 或正常子组件
   */
  render() {
    if (this.state.hasError) {
      // 渲染降级 UI：显示线程加载失败提示
      return (
        <div style={{
          padding: '8px 12px',
          marginLeft: 0,
          marginBottom: 8,
          borderLeft: '3px dashed var(--text-tertiary, #8c8c8c)',
          color: 'var(--text-secondary, #595959)',
          fontSize: 12,
          background: 'var(--bg-secondary, #f5f5f5)',
          borderRadius: 4,
        }}>
          线程加载失败
        </div>
      );
    }
    // 无错误时正常渲染子组件
    return this.props.children;
  }
}

/**
 * InlineThread 组件
 *
 * @param {Object} props - 组件属性
 * @param {Object} props.thread - 线程对象 { id, anchorFingerprint, anchorText, orphaned, messages }
 * @param {boolean} props.collapsed - 是否折叠状态
 * @param {Function} props.onToggle - 切换折叠状态的回调 (threadId) => void
 * @param {Function} props.onDelete - 删除线程的回调 (threadId) => void
 * @param {boolean} props.isStreaming - 是否正在流式回复中
 * @param {string} props.chatSearchKeyword - 聊天搜索关键词（用于显示提示）
 * @param {Function} [props.onActivate] - 追问线程激活回调 (threadId) => void
 */
const InlineThread = ({
  thread,
  collapsed,
  onToggle,
  onDelete,
  isStreaming,
  chatSearchKeyword = '',
  onActivate = null, // 追问线程激活回调 (threadId) => void
  // 子线程相关 props
  onSubThreadAction = null,
  isSubThreadStreamingById = null,
  isSubThreadCollapsed = null,
  nestingLevel = 0,
  onMessageDelete,
  onMessageEdit,
}: {
  thread: any;
  collapsed?: boolean;
  onToggle?: (threadId: string) => void;
  onDelete?: (threadId: string) => void;
  isStreaming?: boolean;
  chatSearchKeyword?: string;
  onActivate?: ((threadId: string) => void) | null;
  onSubThreadAction?: ((...args: any[]) => void) | null;
  isSubThreadStreamingById?: ((threadId: string) => boolean) | null;
  isSubThreadCollapsed?: ((threadId: string) => boolean) | null;
  nestingLevel?: number;
  onMessageDelete?: (messageId: any, shouldPairDelete: any) => void;
  onMessageEdit?: (messageId: any, newContent: any) => void;
}) => {
  /**
   * 获取 Ant Design App 上下文中的 modal 和 message 实例
   * 必须使用 App.useApp() 而不是静态 Modal.confirm / message.xxx，
   * 因为静态方法在 ConfigProvider 外部渲染，无法继承深色主题
   */
  const { modal, message: antMessage } = App.useApp();

  /**
   * 本地状态：删除按钮加载状态
   * 用于防止用户快速点击多次删除按钮
   */
  const [deleteLoading, setDeleteLoading] = useState(false);

  /**
   * 自适应缩进补偿
   *
   * 追问线程可能被渲染在列表（ul/ol）或引用（blockquote）内部，
   * 继承了这些元素的 padding-left 缩进（列表 24px/层，引用 12px）。
   * 通过测量实际偏移量并应用负 margin，使追问线程始终与消息内容区域左对齐，
   * 充分利用水平空间。
   */
  const wrapperRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = wrapperRef.current;
    if (!el) return;
    const markdownRoot = el.closest('.markdown-content');
    if (!markdownRoot) return;
    const rootLeft = markdownRoot.getBoundingClientRect().left;
    const elLeft = el.getBoundingClientRect().left;
    const offset = elLeft - rootLeft;
    if (offset > 2) {
      el.style.marginLeft = `-${offset}px`;
      el.style.width = `calc(100% + ${offset}px)`;
    }
  }, []);

  /**
   * 计算线程内的消息数量
   * 用于显示"N 条追问"和判断是否达到消息上限
   */
  const messageCount = thread.messages?.length || 0;

  /**
   * 判断是否达到消息上限
   * 达到上限时显示提示信息
   */
  const isAtLimit = messageCount >= MAX_THREAD_MESSAGES;

  /**
   * 处理删除按钮点击
   *
   * 使用 Ant Design Modal.confirm 进行二次确认，防止误删
   * 无论线程有多少条消息，都统一显示确认对话框
   */
  const handleDeleteClick = useCallback(() => {
    modal.confirm({
      title: '确认删除追问？',
      content: `删除后将无法恢复此追问线程（包含 ${messageCount} 条消息）`,
      okText: '删除',
      okType: 'danger',
      cancelText: '取消',
      icon: <ExclamationCircleOutlined />,
      onOk: async () => {
        // 用户确认后执行删除操作
        try {
          setDeleteLoading(true); // 设置加载状态，防止重复点击
          onDelete?.(thread.id); // 调用父组件传入的删除回调
        } catch (err) {
          // 删除失败时显示错误提示
          antMessage.error('删除追问失败: ' + ((err as Error)?.message || '未知错误'));
        } finally {
          setDeleteLoading(false); // 清除加载状态
        }
      },
    });
  }, [thread.id, messageCount, onDelete, modal, antMessage]);

  /**
   * 处理折叠/展开切换
   *
   * 点击折叠态的头部区域时触发，展开线程内容
   */
  const handleToggle = useCallback(() => {
    onToggle?.(thread.id);
  }, [thread.id, onToggle]);

  /**
   * 构建折叠态显示文本
   *
   * 根据线程状态（正常/失效）显示不同格式：
   * - 正常: ▶ 💬 N 条追问 (关于「{anchorText}」)
   * - 失效: ▶ ⚠️ N 条追问 (关于「{anchorText}」) - 灰色文字
   */
  const collapsedText = useMemo(() => {
    // 截断过长的锚定文本，避免折叠态占用过多空间
    const truncatedAnchor = thread.anchorText?.length > 30
      ? thread.anchorText.slice(0, 30) + '...'
      : thread.anchorText || '选定文本';

    const levelLabel = nestingLevel > 0 ? '二级追问' : '追问';

    // 失效线程使用警告图标和灰色样式
    if (thread.orphaned) {
      return `▶ ⚠️ ${messageCount} 条${levelLabel} (关于「${truncatedAnchor}」)`;
    }
    // 正常线程使用对话图标
    return `▶ 💬 ${messageCount} 条${levelLabel} (关于「${truncatedAnchor}」)`;
  }, [thread.anchorText, thread.orphaned, messageCount, nestingLevel]);

  /**
   * 渲染线程头部
   *
   * 包含：
   * - 折叠态的点击展开区域（collapsed 时显示）
   * - 展开态的锚定文本展示（!collapsed 时显示）
   * - 删除按钮（始终显示）
   */
  const renderHeader = () => {
    // 折叠态：显示可点击的折叠提示文本
    if (collapsed) {
      return (
        <div
          onClick={handleToggle}
          style={{
            padding: '6px 8px',
            marginLeft: 0,
            marginBottom: 4,
            cursor: 'pointer',
            fontSize: 12,
            color: thread.orphaned
              ? 'var(--text-tertiary, #8c8c8c)'
              : 'var(--text-secondary, #595959)',
            borderRadius: 4,
            transition: 'background-color 0.2s',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor = 'var(--bg-secondary, #f5f5f5)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = 'transparent';
          }}
        >
          {collapsedText}
        </div>
      );
    }

    // 展开态：仅显示锚定文本提示，收起和删除通过右键菜单操作
    return (
      <div style={{
        padding: '4px 0',
        marginBottom: 4,
        marginLeft: 0,
        fontSize: 12,
        color: thread.orphaned
          ? 'var(--text-tertiary, #8c8c8c)'
          : 'var(--text-secondary, #595959)',
      }}>
        关于「{thread.anchorText || '选定文本'}」的{nestingLevel > 0 ? '二级追问' : '追问'}
      </div>
    );
  };

  /**
   * 渲染失效线程提示横幅
   *
   * 仅在线程失效（orphaned=true）且展开时显示
   * 提示用户原始回答已变更，AI 将基于最新回答回答追问
   */
  const renderOrphanedBanner = () => {
    if (!thread.orphaned || collapsed) return null;

    return (
      <div style={{
        marginLeft: 0,
        marginBottom: 8,
        padding: '6px 10px',
        background: 'var(--bg-warning-tint, #fffbe6)',
        border: '1px solid var(--border-warning, #ffe58f)',
        borderRadius: 4,
        fontSize: 11,
        color: 'var(--text-warning, #ad6800)',
        display: 'flex',
        alignItems: 'center',
        gap: 6,
      }}>
        <ExclamationCircleOutlined />
        <span>原始回答已变更，AI 将基于最新回答回答你的追问</span>
      </div>
    );
  };

  /**
   * Phase 6: 移除搜索限制提示横幅
   *
   * 原实现：显示"搜索仅匹配主消息内容"提示
   * Phase 6 改进：搜索已支持线程内容，不再需要此提示
   *
   * 保留空函数注释以说明设计变更（如未来需要恢复提示）
   */

  /**
   * 渲染展开态的线程内容
   *
   * 使用 grid-template-rows: 0fr → 1fr 实现平滑展开动画
   * 这种方式比 max-height 更可靠，不会因为内容高度不确定而出现截断
   */
  const renderExpandedContent = () => {
    if (collapsed) return null;

    return (
      <div
        style={{
          display: 'grid',
          gridTemplateRows: '1fr',
          transition: 'grid-template-rows 0.3s ease-out',
        }}
      >
        <div
          style={{
            overflow: 'hidden',
            // 左边框：顶层线程蓝色粗边框，子线程绿色细边框
            borderLeft: thread.orphaned
              ? '3px dashed var(--border-color-secondary, #cccccc)'
              : nestingLevel > 0
                ? '2px solid var(--status-success)'
                : '3px solid var(--color-primary)',
            marginLeft: 0,
            paddingLeft: 8,
            paddingBottom: 8,
          }}
          className={isStreaming && !thread.orphaned
            ? (nestingLevel > 0 ? 'inline-thread-streaming inline-thread-streaming-green' : 'inline-thread-streaming')
            : undefined}
          role="region"
          aria-label={`追问线程，关于 ${thread.anchorText || '选定文本'}`}
          onContextMenu={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            if (!isAtLimit) {
              onActivate?.(thread.id);
            }
          }}
        >
          {/* 失效线程提示横幅 */}
          {renderOrphanedBanner()}

          {/* 消息列表 */}
          <div style={{ marginBottom: 8, fontSize: nestingLevel > 0 ? 12 : 13 }}>
            {thread.messages.map((msg: any, idx: number) => (
                <ThreadMessageItem
                  key={msg.id || idx}
                  msg={msg}
                  chatSearchKeyword={chatSearchKeyword}
                  onCollapseThread={handleToggle}
                  onDeleteThread={handleDeleteClick}
                  {...(msg.role === 'assistant' ? {
                    subThreads: msg.threads as any[],
                    parentThreadId: thread.id as string,
                    onSubThreadAction: onSubThreadAction as any,
                    isSubThreadStreamingById: isSubThreadStreamingById as any,
                    isSubThreadCollapsed: isSubThreadCollapsed as any,
                  } : {})}
                />
            ))}
          </div>

          {isAtLimit && (
            <div style={{
              padding: '6px 10px',
              fontSize: 11,
              color: 'var(--text-tertiary, #8c8c8c)',
              background: 'var(--bg-secondary, #f5f5f5)',
              borderRadius: 4,
              textAlign: 'center',
            }}>
              已达到消息上限 ({messageCount}/{MAX_THREAD_MESSAGES})
            </div>
          )}
        </div>
      </div>
    );
  };

  /**
   * Phase 6: 处理键盘事件（无障碍访问支持）
   *
   * 支持的键盘快捷键：
   * - Escape: 收起当前聚焦的线程（如果展开的话）
   * - Tab: 正常焦点导航（由浏览器默认处理）
   *
   * @param {KeyboardEvent} e - 键盘事件对象
   */
  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    // Escape 键：收起当前线程
    if (e.key === 'Escape' && !collapsed) {
      e.preventDefault(); // 阻止默认行为
      onToggle?.(thread.id); // 切换到折叠状态
    }
  }, [collapsed, thread.id, onToggle]);

  /**
   * 使用 ErrorBoundary 包裹整个组件
   * 任何渲染错误都会显示降级 UI，不会影响主消息的显示
   *
   * Phase 6 无障碍增强：
   * - role="region": 标识线程为独立区域
   * - aria-label: 为屏幕阅读器提供描述
   * - aria-expanded: 标识展开/折叠状态
   * - onKeyDown: 处理键盘快捷键（Escape 收起）
   */
  return (
    <ThreadErrorBoundary>
      <div
        ref={wrapperRef}
        className="inline-thread-wrapper"
        role="region" // Phase 6: 标识为独立区域，便于屏幕阅读器导航
        aria-label={`追问线程：${thread.anchorText || '选定文本'}，包含 ${messageCount} 条消息`} // Phase 6: 区域描述
        aria-expanded={!collapsed} // Phase 6: 展开状态标识
        onKeyDown={handleKeyDown} // Phase 6: 键盘事件处理
      >
        {/* 头部：折叠/展开控制 */}
        {renderHeader()}

        {/* 展开的内容区域 */}
        {renderExpandedContent()}
      </div>
    </ThreadErrorBoundary>
  );
};

/**
 * React.memo 自定义比较函数
 *
 * 为什么不用默认浅比较？
 * - thread 对象是嵌套结构，浅比较 thread 本身的引用变化即可
 * - 只需比较关键属性：消息数量、最后消息内容、流式状态、失效状态
 * - 避免不必要的重渲染，提升性能
 *
 * @param {Object} prevProps - 上一次的 props
 * @param {Object} nextProps - 这一次的 props
 * @returns {boolean} true 表示 props 没有变化，不需要重渲染
 */
const arePropsEqual = (prevProps: any, nextProps: any) => {
  const prevThread = prevProps.thread;
  const nextThread = nextProps.thread;

  // 比较关键属性：
  // 1. 消息数量变化（新增/删除消息）
  const prevMessageCount = prevThread.messages?.length || 0;
  const nextMessageCount = nextThread.messages?.length || 0;
  if (prevMessageCount !== nextMessageCount) return false;

  // 2. 最后一条消息的内容变化（流式更新时）
  const prevLastMsg = prevThread.messages?.[prevMessageCount - 1];
  const nextLastMsg = nextThread.messages?.[nextMessageCount - 1];
  if (prevLastMsg?.content !== nextLastMsg?.content) return false;

  // 3. 子线程变化（嵌套追问的创建/删除）
  //    子线程挂在 assistant 消息的 threads 数组上，
  //    不改变消息数量和内容，必须单独检查
  const prevMsgs = prevThread.messages || [];
  const nextMsgs = nextThread.messages || [];
  for (let i = 0; i < prevMessageCount; i++) {
    if (prevMsgs[i]?.threads !== nextMsgs[i]?.threads) return false;
  }

  // 4. 流式状态变化（开始/结束流式）
  if (prevProps.isStreaming !== nextProps.isStreaming) return false;

  // 5. 失效状态变化（消息重新生成）
  if (prevThread.orphaned !== nextThread.orphaned) return false;

  // 6. 折叠状态变化
  if (prevProps.collapsed !== nextProps.collapsed) return false;

  // 7. 搜索关键词变化（影响提示横幅显示）
  if (prevProps.chatSearchKeyword !== nextProps.chatSearchKeyword) return false;

  // 8. 激活回调引用变化
  if (prevProps.onActivate !== nextProps.onActivate) return false;

  // 9. 子线程相关 props 变化
  if (prevProps.nestingLevel !== nextProps.nestingLevel) return false;
  if (prevProps.onSubThreadAction !== nextProps.onSubThreadAction) return false;
  if (prevProps.isSubThreadStreamingById !== nextProps.isSubThreadStreamingById) return false;
  if (prevProps.isSubThreadCollapsed !== nextProps.isSubThreadCollapsed) return false;

  // 所有关键属性都没变化，不需要重渲染
  return true;
};

export default React.memo(InlineThread, arePropsEqual);

/**
 * Phase 6: 导出 ThreadGroupEntry 组件
 * 供 MarkdownRenderer 在多线程合并场景下使用
 */
export { ThreadGroupEntry };
