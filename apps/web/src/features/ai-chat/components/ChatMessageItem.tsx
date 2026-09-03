/**
 * ChatMessageItem.jsx
 * ============================================================
 * JayRead AI Chat 单条消息渲染组件
 * 从 ChatPanel 中提取的独立组件
 *
 * 核心功能：
 * - 渲染单条聊天消息（用户消息/AI消息）
 * - 支持右键菜单（引用消息/删除消息）
 * - 支持搜索关键词高亮
 * - 支持多模态消息（文本 + 图片）
 * - 网络搜索来源显示
 * - 支持选中文字创建追问线程（Phase 4 新增）
 *
 * 技术栈：
 * - Ant Design：Dropdown、Spin、Button、Modal
 * - MarkdownRenderer：AI 消息格式化渲染
 * ============================================================
 */

import React, { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { Dropdown, Spin, Button, App } from 'antd';
import { DeleteOutlined, CopyOutlined, MessageOutlined } from '@ant-design/icons';
import ToolCallCard from './ToolCallCard';
import MarkdownRenderer from './MarkdownRenderer';
import { extractTextFromContent, extractImageUrlsFromContent, createKeywordHighlighter } from '../utils/chatMessageUtils';


/**
 * 浮动按钮容器宽度（像素）
 *
 * 包含一个按钮："💬 追问"
 * - 每个按钮宽度约 60px
 * 用于计算浮动按钮的位置，确保不超出消息容器
 */
const FLOATING_BUTTON_CONTAINER_WIDTH = 60;

/**
 * 流式完成后延迟时间（毫秒）
 *
 * 为什么需要延迟？
 * - 确保流式完成后 MarkdownRenderer 已将最新的 data-source-start/end 注入 DOM
 * - 200ms 足够 React 完成重渲染，但不会让用户感觉卡顿
 */
const STREAMING_END_DELAY = 200;

/**
 * ChatMessageItem 组件
 * 渲染单条聊天消息，包括消息气泡、右键菜单、搜索来源、追问线程等
 *
 * @param {Object} props - 组件属性
 * @param {Object} props.msg - 消息对象 { role, content, threads }
 * @param {number} props.originalIdx - 消息在原始 messages 数组中的索引
 * @param {boolean} props.streaming - 是否正在流式回复中（主对话）
 * @param {boolean} props.isLastMessage - 是否是最后一条消息
 * @param {string} props.chatSearchKeyword - 搜索关键词（用于高亮）
 * @param {number} props.paperId - 论文 ID
 * @param {Function} props.onQuoteMessage - 引用消息的回调
 * @param {Function} props.onDeleteMessage - 删除消息的回调
 * @param {Function} props.onSearchResultClick - 搜索结果点击的回调
 * @param {boolean} props.isMainStreaming - 主对话是否正在流式回复中
 * @param {boolean} props.isThreadStreaming - 是否有线程正在流式回复中
 * @param {number} props.threadCount - 该消息包含的线程数量
 * @param {Function} props.onCreateThread - 创建追问线程的回调 (msgIdx, { fingerprint, anchorText }) => void
 * @param {Array} props.threads - 该消息关联的线程数组（Phase 5: 线程 UI 完整集成）
 * @param {Function} props.onDeleteThread - 删除线程回调 (msgIdx, threadId) => void
 * @param {Function} props.onToggleThreadCollapse - 切换线程折叠状态回调 (msgIdx, threadId) => void
 * @param {Function} props.onCollapseAll - 折叠所有线程回调 (msgIdx) => void
 * @param {Function} props.onExpandAll - 展开所有线程回调 (msgIdx) => void
 * @param {Function} props.onThreadSendMessage - 线程内发送消息回调 (msgIdx, threadId, text) => void
 * @param {Function} props.isThreadCollapsed - 判断线程是否折叠 (threadId) => boolean
 * @param {Function} props.isThreadStreamingById - 判断指定线程是否在流式回复中 (threadId) => boolean（Phase 6：并行流式支持）
 * @param {Function} props.onThreadMessageDelete - 线程内删除消息回调 (msgIdx, threadId, messageId, shouldPairDelete) => void（Phase 6）
 * @param {Function} props.onThreadMessageEdit - 线程内编辑消息回调 (msgIdx, threadId, messageId, newContent) => void（Phase 6）
 */
const ChatMessageItem = ({
    msg,
    originalIdx,
    streaming,
    isLastMessage,
    chatSearchKeyword,
    paperId,
    onQuoteMessage,
    onDeleteMessage,
    onSearchResultClick,
    // 基础线程相关 props
    isMainStreaming = false,
    isThreadStreaming = () => false,       // 判断指定线程是否在流式回复中 (threadId) => boolean
    threadCount = 0,
    onCreateThread = null,
    // Phase 5: 线程 UI 完整集成 props
    threads = [],                          // 该消息关联的线程数组
    onDeleteThread = null,                 // 删除线程回调 (msgIdx, threadId) => void
    onToggleThreadCollapse = null,         // 切换线程折叠状态回调 (msgIdx, threadId) => void
    onCollapseAll = null,                  // 折叠所有线程回调 (msgIdx) => void
    onExpandAll = null,                    // 展开所有线程回调 (msgIdx) => void
    onThreadSendMessage = null,            // 线程内发送消息回调 (msgIdx, threadId, text) => void
    isThreadCollapsed = null,              // 判断线程是否折叠 (threadId) => boolean
    isThreadStreamingById = null,          // Phase 6: 判断指定线程是否在流式回复中 (threadId) => boolean
    onThreadMessageDelete = null,          // Phase 6: 线程内删除消息回调 (msgIdx, threadId, messageId, shouldPairDelete) => void
    onThreadMessageEdit = null,            // Phase 6: 线程内编辑消息回调 (msgIdx, threadId, messageId, newContent) => void
    onActivateThread = null,               // 追问线程激活回调 (threadId) => void
    onDeactivateThread = null,             // 退出追问模式回调 () => void
    // 子线程相关 props
    onSubThreadSendMessage = null,         // 子线程发送消息回调 (msgIdx, parentThreadId, subThreadId, text) => void
    onDeleteSubThread = null,              // 删除子线程回调 (msgIdx, parentThreadId, subThreadId) => void
    onToggleSubThreadCollapse = null,      // 子线程折叠切换回调 (msgIdx, parentThreadId, subThreadId) => void
    onDeleteSubThreadMessage = null,       // 子线程内删除消息回调 (msgIdx, parentThreadId, subThreadId, messageId, shouldPairDelete) => void
    onEditSubThreadMessage = null,         // 子线程内编辑消息回调 (msgIdx, parentThreadId, subThreadId, messageId, newContent) => void
    onActivateSubThread = null,            // 激活子线程回调 (msgIdx, parentThreadId, subThreadId) => void
    onCreateAndActivateSubThread = null,   // 创建并激活子线程回调 (msgIdx, parentThreadId, {fingerprint, anchorText}) => void
}: {
    msg: any;
    originalIdx: number;
    streaming?: any;
    isLastMessage?: boolean;
    chatSearchKeyword?: string;
    paperId?: number;
    onQuoteMessage?: (msg: any) => void;
    onDeleteMessage?: (idx: number) => void;
    onSearchResultClick?: (idx: number) => void;
    isMainStreaming?: boolean;
    isThreadStreaming?: ((threadId: string) => boolean) | (() => false);
    threadCount?: number;
    onCreateThread?: ((msgIdx: number, data: any) => void) | null;
    threads?: any[];
    onDeleteThread?: ((msgIdx: number, threadId: string) => void) | null;
    onToggleThreadCollapse?: ((msgIdx: number, threadId: string) => void) | null;
    onCollapseAll?: ((msgIdx: number) => void) | null;
    onExpandAll?: ((msgIdx: number) => void) | null;
    onThreadSendMessage?: ((msgIdx: number, threadId: string, text: string) => void) | null;
    isThreadCollapsed?: ((threadId: string) => boolean) | null;
    isThreadStreamingById?: ((threadId: string) => boolean) | null;
    onThreadMessageDelete?: ((msgIdx: number, threadId: string, messageId: string, shouldPairDelete?: boolean) => void) | null;
    onThreadMessageEdit?: ((msgIdx: number, threadId: string, messageId: string, newContent: string) => void) | null;
    onActivateThread?: ((threadId: string) => void) | null;
    onDeactivateThread?: (() => void) | null;
    onSubThreadSendMessage?: ((msgIdx: number, parentThreadId: string, subThreadId: string, text: string) => void) | null;
    onDeleteSubThread?: ((msgIdx: number, parentThreadId: string, subThreadId: string) => void) | null;
    onToggleSubThreadCollapse?: ((msgIdx: number, parentThreadId: string, subThreadId: string) => void) | null;
    onDeleteSubThreadMessage?: ((msgIdx: number, parentThreadId: string, subThreadId: string, messageId: string, shouldPairDelete?: boolean) => void) | null;
    onEditSubThreadMessage?: ((msgIdx: number, parentThreadId: string, subThreadId: string, messageId: string, newContent: string) => void) | null;
    onActivateSubThread?: ((msgIdx: number, parentThreadId: string, subThreadId: string) => void) | null;
    onCreateAndActivateSubThread?: ((msgIdx: number, parentThreadId: string, data: any) => void) | null;
}) => {
    // 获取 Ant Design App 上下文中的 modal/message 实例
    // 必须使用 App.useApp() 而不是静态方法，
    // 因为静态方法在 ConfigProvider 外部渲染，无法继承深色主题
    const { modal, message } = App.useApp();

    // ========== 状态管理 ==========

    /**
     * 选中的文字状态
     *
     * 存储用户在 AI 消息中选中的文字内容。
     */
    const [selectedText, setSelectedText] = useState(''); // 选中的文字，初始为空字符串

    /**
     * 缓存选中文字所在段落的 fingerprint
     *
     * 在 mouseup 时（processTextSelection）从 DOM 读取并缓存。
     * 这样右键菜单点击时不需要再读取 window.getSelection()（选区可能已被清除）。
     */
    const selectedFingerprintRef = useRef<string | null>(null);

    /**
     * 浮动按钮位置状态
     *
     * 存储浮动按钮的坐标位置，用于在选中文字附近显示。
     * 两个按钮共用此位置状态。
     */
    const [floatingButtonPos, setFloatingButtonPos] = useState<{ top: number; left: number } | null>(null); // 浮动按钮位置，初始为 null

    /**
     * 流式完成时间戳
     *
     * 仅在流式真正结束时设置（通过 wasStreamingRef 检测状态转换）。
     * 从历史加载的消息此值为 null，canCreateThread 直接返回 true。
     */
    const [streamingEndTime, setStreamingEndTime] = useState<number | null>(null);

    /**
     * 流式完成后是否已过延迟期
     *
     * 仅当检测到 streaming→not streaming 转换时，通过 setTimeout 延迟设置。
     * 避免 useMemo 中使用 Date.now() 导致缓存过期的问题。
     */
    const [streamingReady, setStreamingReady] = useState(false);

    /**
     * 追踪组件是否在本会话中经历过流式
     *
     * 用于区分"从历史加载的消息"和"刚结束流式的消息"：
     * - 加载的消息：isMainStreaming 始终为 false，wasStreamingRef 始终为 false
     * - 流式消息：isMainStreaming 从 true 变为 false，wasStreamingRef 记录了这个转换
     *
     * 这避免了给历史加载的消息施加不必要的 200ms 延迟。
     */
    const wasStreamingRef = useRef(isMainStreaming);

    /**
     * 是否跨越多个段落选择
     *
     * 当用户选择跨越多个块级元素（段落、列表等）时设为 true
     * 用于显示提示"将在线程中引用完整选中内容"
     */
    const [isCrossParagraphSelection, setIsCrossParagraphSelection] = useState(false);

    /**
     * 消息容器引用
     *
     * 用于获取消息容器的 DOM 元素，计算浮动按钮位置。
     */
    const messageContainerRef = useRef<HTMLDivElement>(null); // 消息容器引用

    /**
     * 浮动按钮容器引用
     *
     * 用于动态获取按钮容器的实际宽度
     */
    const floatingButtonContainerRef = useRef<HTMLDivElement>(null);

    // ========== 计算属性 ==========

    /**
     * 是否可以创建线程
     *
     * 判断逻辑：
     * - 线程流式时禁用（isThreadStreaming）
     * - 主对话流式时允许对已完成消息创建线程
     * - 必须在流式完成后延迟一段时间（确保 DOM 已更新）
     * - 必须传入 onCreateThread 回调
     *
     * 设计说明：
     * - MVP 优化：已完成消息允许创建线程
     * - 主对话流式时用户可对已完成消息追问（data 属性已注入）
     * - 流式完成后延迟 200ms 确保 MarkdownRenderer 已注入 data 属性
     */
    const canCreateThread = useMemo(() => {
        // 线程流式时禁用
        // 注意：useThreadManager 的返回对象中，布尔值 isThreadStreaming 被同名函数覆盖，
        // 所以这里收到的 isThreadStreaming 实际是 (threadId) => boolean 函数。
        // 需要通过检查 msg.threads 中的每个线程来判断是否正在流式。
        const threadStreamingActive = typeof isThreadStreaming === 'function'
            ? (msg.threads || []).some((t: any) => (isThreadStreaming as any)(t.id))
            : isThreadStreaming;
        if (threadStreamingActive) return false;

        if (!onCreateThread) return false;

        if (isMainStreaming) {
            if (isLastMessage) return false;
            return true;
        }

        // 流式刚结束时等待延迟（确保 DOM data 属性已注入）
        if (streamingEndTime) {
            return streamingReady;
        }

        // 没有流式记录（从历史加载的消息），允许创建线程
        return true;
    }, [isThreadStreaming, msg.threads, onCreateThread, isMainStreaming, isLastMessage, streamingEndTime, streamingReady]);

    /**
     * 是否显示浮动按钮
     *
     * 判断逻辑：
     * - 有选中的文字
     * - 选中文字长度 > 5 个字符（避免误触）
     * - 不在流式禁用期间（canCreateThread 为 true）
     *
     * 设计说明：
     * - 流式期间 mouseup 仍触发并计算位置，但按钮不渲染
     * - 这样可以避免流式结束后按钮位置跳动
     */
    const shouldShowFloatingButtons = useMemo(() => {
        return selectedText.length > 5 && canCreateThread;
    }, [selectedText.length, canCreateThread]);

    // ========== 处理函数 ==========

    /**
     * 处理鼠标抬起事件
     *
     * 检测用户是否选中了文字，如果是则计算浮动按钮位置。
     *
     * 使用原生 mouseup 事件而不是 React 的 onMouseUp，因为：
     * - 需要获取 window.getSelection() 来获取选中的文字
     * - React 的事件系统对文字选中的支持有限
     *
     * 流式期间的特殊处理：
     * - mouseup 仍触发并计算位置（避免流式结束后按钮位置跳动）
     * - 但按钮不渲染（canCreateThread 为 false）
     *
     * @param {MouseEvent} e - 鼠标事件对象
     */
    const handleMouseUp = useCallback((e: MouseEvent) => {
        // 只处理 AI 消息中的文字选中（用户消息不添加此功能）
        if (msg.role !== 'assistant') return;

        processTextSelection();
    }, [msg.role]);

    /**
     * Phase 6: 处理触摸结束事件（触摸设备支持）
     *
     * 在触摸设备（手机、平板）上支持文字选择后显示追问按钮。
     *
     * 设计说明：
     * - 使用 touchend 事件监听触摸结束
     * - 延迟 200ms 判断：区分触摸选择和滚动操作
     * - 如果触摸期间移动距离超过 10px，认为是滚动而非选择
     * - 触摸坐标用于定位浮动按钮（touch 事件的 clientX/Y）
     *
     * @param {TouchEvent} e - 触摸事件对象
     */
    const handleTouchEnd = useCallback((e: TouchEvent) => {
        // 只处理 AI 消息中的文字选中
        if (msg.role !== 'assistant') return;

        // 获取触摸点的起始位置（从 touches 中获取）
        const touch = e.changedTouches[0];
        if (!touch) return;

        // 延迟执行，等待浏览器完成文字选择
        // 同时区分触摸选择和滚动：短时间内检查是否有文字被选中
        setTimeout(() => {
            processTextSelection();
        }, 200);
    }, [msg.role]);

    /**
     * Phase 6: 处理文字选择的通用逻辑
     *
     * 统一处理鼠标和触摸设备上的文字选择检测。
     * 提取为独立函数，避免 mouseup 和 touchend 事件处理代码重复。
     *
     * 功能：
     * - 获取选中的文字内容
     * - 计算浮动按钮位置
     * - 检测是否跨段落选择
     * - 更新相关状态
     */
    const processTextSelection = useCallback(() => {
        // 延迟执行，确保浏览器已完成文字选择
        setTimeout(() => {
            const selection = window.getSelection();
            const text = selection?.toString().trim();

            // 如果有选中文字且长度大于 5 个字符（避免误触）
            if (text && text.length > 5) {
                setSelectedText(text);

                // 获取选中的范围
                const range = selection!.getRangeAt(0);
                const rect = range.getBoundingClientRect();
                const containerRect = messageContainerRef.current?.getBoundingClientRect();

                // 计算相对于消息容器的位置
                if (containerRect) {
                    // 获取按钮容器的实际宽度（如果已渲染）
                    const buttonContainerWidth = floatingButtonContainerRef.current
                        ? floatingButtonContainerRef.current.offsetWidth
                        : FLOATING_BUTTON_CONTAINER_WIDTH;

                    // 计算左侧位置（水平居中）
                    let left = rect.left - containerRect.left + rect.width / 2 - buttonContainerWidth / 2;

                    // 边界修正：确保按钮不超出容器左侧
                    if (left < 0) left = 0;

                    // 边界修正：确保按钮不超出容器右侧
                    if (left + buttonContainerWidth > containerRect.width) {
                        left = containerRect.width - buttonContainerWidth;
                    }

                    setFloatingButtonPos({
                        top: rect.top - containerRect.top - 40, // 在选中文字上方 40px
                        left: left, // 使用计算后的位置（带边界修正）
                    });

                    // 检查是否跨段落选择
                    // 向上查找 data-source-start 属性的元素
                    const anchorElement = range.startContainer.parentElement
                        ?.closest('[data-source-start]');

                    // 如果找不到锚定元素，可能是 KaTeX 内部节点或其他不支持的情况
                    if (!anchorElement) {
                        // 不设置 isCrossParagraphSelection，而是保持 false
                        // 点击时会检查并提示"该段落暂不支持追问"
                        setIsCrossParagraphSelection(false);
                        selectedFingerprintRef.current = null;
                    } else {
                        // 缓存 fingerprint（右键菜单点击时 DOM 选区可能已消失）
                        selectedFingerprintRef.current = anchorElement.getAttribute('data-block-fingerprint');

                        // 检查选区是否完全在同一个元素内
                        const startElement = range.startContainer.parentElement;
                        const endElement = range.endContainer.parentElement;
                        const startAnchor = startElement?.closest('[data-source-start]');
                        const endAnchor = endElement?.closest('[data-source-start]');

                        // 如果起始和结束的锚定元素不同，说明跨段落选择
                        setIsCrossParagraphSelection(
                            !!(startAnchor && endAnchor && startAnchor !== endAnchor)
                        );
                    }
                }
            } else {
                // 没有选中文字或文字太短
                setSelectedText('');
                setFloatingButtonPos(null);
                setIsCrossParagraphSelection(false);
                selectedFingerprintRef.current = null;
            }
        }, 10);
    }, []);

    /**
     * 处理点击消息容器
     *
     * 用户点击消息容器时，隐藏浮动按钮（取消选择）。
     */
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

    /**
     * 处理点击"追问"浮动按钮
     *
     * 创建新的追问线程：
     * 1. 查找锚定元素（向上查找 [data-source-start]）
     * 2. 读取 fingerprint 和 anchorText
     * 3. 调用 onCreateThread 回调
     * 4. 隐藏浮动按钮
     */
    const handleCreateThread = useCallback(() => {
        // 优先使用 mouseup 时缓存的 fingerprint
        // 右键菜单场景下 DOM 选区可能已被清除，无法实时获取
        const fingerprint = selectedFingerprintRef.current;
        if (!fingerprint) {
            // 降级：尝试从当前 DOM 选区读取
            const selection = window.getSelection();
            if (!selection || selection.rangeCount === 0) {
                message.warning('请先选中要追问的文字（至少 5 个字符）');
                return;
            }
            const range = selection.getRangeAt(0);
            const anchorElement = range.startContainer.parentElement
                ?.closest('[data-source-start]');
            if (!anchorElement) {
                message.warning('该段落暂不支持追问');
                setFloatingButtonPos(null);
                return;
            }
            const fallbackFingerprint = anchorElement.getAttribute('data-block-fingerprint');
            if (!fallbackFingerprint) {
                message.warning('无法获取段落指纹，请刷新页面后重试');
                setFloatingButtonPos(null);
                return;
            }
            selectedFingerprintRef.current = fallbackFingerprint;
        }

        // 使用缓存的 fingerprint（降级后也已更新）
        if (onCreateThread) {
            onCreateThread(originalIdx, {
                fingerprint: selectedFingerprintRef.current,
                anchorText: selectedText,
            });
        }

        // 隐藏浮动按钮
        setFloatingButtonPos(null);
        // 清空选中状态
        setIsCrossParagraphSelection(false);
    }, [originalIdx, selectedText, onCreateThread]);

    /**
     * 线程操作统一分发器
     *
     * 将 MarkdownRenderer 的统一回调接口 (type, threadId, ...args)
     * 分发到 useThreadManager 提供的具体操作方法。
     *
     * 设计原因：
     * - MarkdownRenderer 不直接感知 useThreadManager 的 API
     * - 通过统一接口解耦，ChatMessageItem 负责适配
     * - 支持的操作类型：
     *   - 'toggle': 切换线程折叠状态
     *   - 'delete': 删除线程
     *   - 'send': 发送线程消息
     *   - 'messageDelete': 删除线程内单条消息（Phase 6）
     *   - 'messageEdit': 编辑线程内单条消息（Phase 6）
     *
     * @param {string} type - 操作类型
     * @param {string} threadId - 线程 ID
     * @param {...*} args - 额外参数（如 send 操作时的消息文本）
     */
    const handleThreadAction = useCallback((type: string, threadId: string, ...args: any[]) => {
        switch (type) {
            case 'toggle':
                // 切换线程折叠状态
                onToggleThreadCollapse?.(originalIdx, threadId);
                break;
            case 'delete':
                // 删除线程
                onDeleteThread?.(originalIdx, threadId);
                break;
            case 'send':
                // 发送线程消息，args[0] 是消息文本
                onThreadSendMessage?.(originalIdx, threadId, args[0]);
                break;
            case 'messageDelete':
                // Phase 6: 删除线程内单条消息
                // args[0] = messageId, args[1] = shouldPairDelete
                onThreadMessageDelete?.(originalIdx, threadId, args[0], args[1]);
                break;
            case 'messageEdit':
                // Phase 6: 编辑线程内单条消息
                // args[0] = messageId, args[1] = newContent
                onThreadMessageEdit?.(originalIdx, threadId, args[0], args[1]);
                break;
            // 子线程操作
            // wrappedSubThreadAction 将 parentThreadId 注入 threadId 位置：
            // handleThreadAction(type, threadId=parentThreadId, ...args=[subThreadId, ...rest])
            case 'createSubThread':            // nestingLevel=0 直接路径（未经过 SUB_THREAD_TYPE_MAP 映射）
            case 'createAndActivateSubThread': // nestingLevel=1 经过 wrappedSubThreadAction 映射的路径
                onCreateAndActivateSubThread?.(originalIdx, threadId, args[0]);
                break;
            case 'subThreadSend':
                onSubThreadSendMessage?.(originalIdx, threadId, args[0], args[1]);
                break;
            case 'subThreadDelete':
                onDeleteSubThread?.(originalIdx, threadId, args[0]);
                break;
            case 'subThreadToggle':
                onToggleSubThreadCollapse?.(originalIdx, threadId, args[0]);
                break;
            case 'subThreadMessageDelete':
                onDeleteSubThreadMessage?.(originalIdx, threadId, args[0], args[1], args[2]);
                break;
            case 'subThreadMessageEdit':
                onEditSubThreadMessage?.(originalIdx, threadId, args[0], args[1], args[2]);
                break;
            case 'subThreadActivate':
                onActivateSubThread?.(originalIdx, threadId, args[0]);
                break;
            default:
                console.warn(`[ChatMessageItem] 未知线程操作类型: ${type}`);
        }
    }, [originalIdx, onToggleThreadCollapse, onDeleteThread, onThreadSendMessage, onThreadMessageDelete, onThreadMessageEdit, onCreateAndActivateSubThread, onSubThreadSendMessage, onDeleteSubThread, onToggleSubThreadCollapse, onDeleteSubThreadMessage, onEditSubThreadMessage, onActivateSubThread]);

    // ========== 副作用处理 ==========

    /**
     * Phase 6: 添加/移除 mouseup 和 touchend 事件监听器
     *
     * 组件挂载时添加监听器，卸载时移除监听器。
     * - mouseup: 鼠标设备文字选择事件
     * - touchend: 触摸设备文字选择事件
     *
     * 触摸支持说明：
     * - touchend 延迟 200ms 执行，区分选择和滚动
     * - 使用与 mouseup 相同的选择处理逻辑
     */
    useEffect(() => {
        // 只对 AI 消息添加监听器
        if (msg.role === 'assistant' && messageContainerRef.current) {
            const container = messageContainerRef.current;
            container.addEventListener('mouseup', handleMouseUp);
            container.addEventListener('touchend', handleTouchEnd); // Phase 6: 触摸设备支持

            // 返回清理函数
            return () => {
                container.removeEventListener('mouseup', handleMouseUp);
                container.removeEventListener('touchend', handleTouchEnd); // Phase 6: 触摸设备支持
            };
        }
    }, [msg.role, handleMouseUp, handleTouchEnd]);

    /**
     * 监听流式状态变化
     *
     * 使用 wasStreamingRef 检测 streaming→not streaming 的状态转换，
     * 只对"刚结束流式"的消息施加延迟，不对历史加载的消息施加。
     *
     * 关键设计：
     * - effect 仅依赖 isMainStreaming，不会被自身触发的状态更新反复执行
     * - timer 不会在 effect cleanup 时被意外清除
     */
    useEffect(() => {
        // 检测 streaming → not streaming 的转换
        if (wasStreamingRef.current && !isMainStreaming) {
            // 流式刚结束：启动延迟计时器
            setStreamingEndTime(Date.now());
            setStreamingReady(false);
            const timer = setTimeout(() => setStreamingReady(true), STREAMING_END_DELAY);
            wasStreamingRef.current = false;
            return () => clearTimeout(timer);
        }

        if (isMainStreaming) {
            // 正在流式：重置状态
            setStreamingEndTime(null);
            setStreamingReady(false);
        }

        wasStreamingRef.current = isMainStreaming;
    }, [isMainStreaming]);

    const highlightChatKeyword = useCallback(createKeywordHighlighter(chatSearchKeyword as string), [chatSearchKeyword]);

    // 右键菜单配置
    const dropdownMenu: any = {
        items: [
            {
                key: 'quote',
                label: '引用消息',
                icon: <CopyOutlined />,
                onClick: () => {
                    onQuoteMessage?.({
                        role: msg.role,
                        content: extractTextFromContent(msg.content),
                        // Phase 6: 添加消息索引，用于"转为追问"功能
                        originalIdx: originalIdx,
                    });
                },
            },
            {
                type: 'divider',
            },
            {
                key: 'delete',
                label: '删除消息',
                icon: <DeleteOutlined />,
                danger: true,
                onClick: () => {
                    // 如果消息包含线程，显示确认提示
                    const threadText = threadCount > 0
                        ? `（包含 ${threadCount} 个追问线程）`
                        : '';
                    if (threadCount > 0) {
                        modal.confirm({
                            title: '确认删除消息？',
                            content: `删除后将同时删除该消息的所有追问线程${threadText}，无法恢复。`,
                            okText: '删除',
                            okType: 'danger',
                            cancelText: '取消',
                            onOk: () => onDeleteMessage?.(originalIdx),
                        });
                    } else {
                        onDeleteMessage?.(originalIdx);
                    }
                },
            },
        ],
    };

    return (
        <>
            <Dropdown
                key={originalIdx}
                trigger={['contextMenu']}
                menu={dropdownMenu}
            >
                <div
                    ref={messageContainerRef}
                    id={`chat-msg-${originalIdx}`}
                    style={{
                        marginBottom: 12,
                        display: 'flex',
                        flexDirection: 'column',
                        alignItems: msg.role === 'user' ? 'flex-end' : 'flex-start',
                        position: 'relative',
                        ...(chatSearchKeyword?.trim() ? { cursor: 'pointer' } : {}),
                    }}
                    onClick={(e) => {
                        // 如果点击的是浮动按钮，不处理
                        if ((e.target as HTMLElement).closest('.chat-floating-action-btn')) return;
                        // 处理搜索关键词点击或普通点击
                        if (chatSearchKeyword?.trim()) {
                            onSearchResultClick?.(originalIdx);
                        } else {
                            handleContainerClick();
                            // 点击主面板消息内容，退出追问模式
                            onDeactivateThread?.();
                        }
                    }}
                >
                    {/* 消息气泡 */}
                    <div style={{
                        ...(msg.role === 'user' ? { maxWidth: '85%' } : { width: '100%' }),
                        padding: msg.role === 'assistant' ? '4px 0' : '8px 12px',
                        borderRadius: msg.role === 'user' ? '12px 12px 2px 12px' : '0',
                        background: msg.role === 'user' ? 'var(--accent-content, #242424)' : 'transparent',
                        color: msg.role === 'user' ? 'var(--accent-content-text, #fff)' : 'var(--text-primary, #262626)',
                        fontSize: 13,
                        lineHeight: 1.6,
                        whiteSpace: msg.role === 'user' ? 'pre-wrap' : 'normal',
                        wordBreak: msg.role === 'user' ? 'break-word' : 'normal',
                        position: 'relative', // 为线程指示器提供定位上下文
                    }}>

                        {/* 根据消息角色选择渲染方式 */}
                        {msg.role === 'assistant' ? (
                            // AI 消息渲染
                            msg.content === '__WEB_SEARCHING__' ? (
                                // 网络搜索进行中的临时指示器
                                <div style={{
                                    display: 'flex',
                                    alignItems: 'center',
                                    gap: 8,
                                    color: 'var(--text-tertiary, #8c8c8c)',
                                    fontSize: 13,
                                }}>
                                    <Spin size="small" />
                                    <span>Searching the web...</span>
                                </div>
                            ) : msg.content === '' && streaming ? null : (
                                // AI 消息：使用 MarkdownRenderer 渲染格式化内容
                                // Phase 5: 传递线程相关 props，支持内联线程显示
                                // Phase 6: 传递 isThreadStreamingById 函数支持并行流式
                                <MarkdownRenderer
                                    content={msg.content}
                                    searchKeyword={chatSearchKeyword}
                                    threads={threads}
                                    onThreadAction={handleThreadAction}
                                    isThreadStreamingById={isThreadStreamingById ?? undefined}
                                    isThreadStreaming={isThreadStreaming}
                                    isThreadCollapsed={isThreadCollapsed ?? undefined}
                                    onActivateThread={onActivateThread ?? undefined}
                                    onSubThreadAction={handleThreadAction}
                                    isSubThreadStreamingById={isThreadStreamingById ?? undefined}
                                    isSubThreadCollapsed={isThreadCollapsed ?? undefined}
                                />
                            )
                        ) : (
                            // 用户消息：支持纯文本和多模态（文本 + 图片）显示
                            <>
                                {/* 文本内容 */}
                                {highlightChatKeyword(extractTextFromContent(msg.content))}
                                {/* 图片缩略图 */}
                                {extractImageUrlsFromContent(msg.content).map((imgUrl, imgIdx) => (
                                    <img
                                        key={imgIdx}
                                        src={imgUrl}
                                        alt={`附件图片 ${imgIdx + 1}`}
                                        style={{
                                            maxWidth: '100%',
                                            maxHeight: 200,
                                            borderRadius: 6,
                                            marginTop: 4,
                                            display: 'block',
                                        }}
                                    />
                                ))}
                            </>
                        )}
                    </div>

                    {/* Agent tool calls */}
                    {msg.role === 'assistant' && msg.toolCalls?.length > 0 && (
                      <>
                        {msg.toolCalls.map((tc: any, i: number) => (
                          <ToolCallCard key={tc.id || i} toolCall={tc} />
                        ))}
                      </>
                    )}

                    {/* 浮动按钮容器（追问按钮） */}
                    {msg.role === 'assistant' && floatingButtonPos && shouldShowFloatingButtons && (
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
                            {/* 追问按钮（主要操作，primary 样式） */}
                            <Button
                                type="primary"
                                size="small"
                                icon={<MessageOutlined />}
                                className="chat-floating-action-btn"
                                onClick={handleCreateThread}
                                style={{
                                    borderRadius: 4,
                                }}
                            >
                                追问
                            </Button>

                            {/* 跨段落选择提示 */}
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
        </>
    );
};


// ========== 使用 React.memo 优化性能 ==========
// 流式输出时消息列表频繁更新，单条消息 props 不变时不应重渲染
// React.memo 会对 props 进行浅比较，只有 props 变化时才重新渲染
export default React.memo(ChatMessageItem);
