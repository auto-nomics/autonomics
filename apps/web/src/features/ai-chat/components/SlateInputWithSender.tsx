/**
 * SlateInputWithSender.jsx
 * ============================================================
 * JayRead AI Chat 富文本输入编辑器组件
 * 基于 Slate.js 构建的聊天输入框，适配自 Zotero 插件版本
 * 移除了 Zotero 依赖，改为通过 props 接收配置数据和保存回调
 *
 * 核心功能：
 * - 基于 Slate.js 的富文本编辑器（支持撤销/重做历史）
 * - VibeCard @提及支持（可通过输入或拖拽插入文献卡片引用）
 * - 自定义模型配置弹窗（支持多配置管理：添加/编辑/删除/切换）
 * - 文件上传/粘贴/截图（多模态消息支持，图片以 base64 存储）
 * - 附件支持（图片/文本文件/二进制文件，二进制文件后端转 Markdown）
 * - 发送按钮和键盘快捷键（Enter 发送，Shift+Enter 换行）
 * - 多模态消息构建（文本 + VibeCard 引用 + 附件）
 * - Web 搜索开关按钮
 *
 * 技术栈：
 * - Slate.js：富文本编辑器核心
 * - slate-react：Slate 的 React 绑定
 * - slate-history：撤销/重做历史支持
 * - Ant Design：UI 组件库（Button、Modal、Form、Dropdown 等）
 * ============================================================
 */

// React 核心钩子
import React, { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
// Slate.js 核心模块：创建编辑器实例、Editor 工具对象、Range 范围操作、Transforms 变换操作
import { createEditor, Editor, Range, Transforms, Descendant, Node } from 'slate';
// Slate.js 撤销/重做历史插件
import { withHistory } from 'slate-history';
// Slate.js React 绑定：Editable 可编辑组件、Slate 上下文组件、withReact React 插件、render 回调 props 类型
import { Editable, Slate, ReactEditor, withReact, RenderElementProps, RenderLeafProps } from 'slate-react';
// Ant Design UI 组件：按钮、弹性布局、主题、消息提示、弹窗
import { Button, theme, App, Modal, Input } from 'antd';
// 模型配置弹窗组件
import NiceModal from '@ebay/nice-modal-react';
// 国际化工具函数：获取翻译文本
// 附件预览组件（显示在输入框上方，展示待发送的图片缩略图和文件卡片）
import AttachmentPreview from './AttachmentPreview';
// 文件搜索弹出组件
import type { CustomModelConfig, ModelSelection } from '../hooks/useModelConfig';
// Slate 编辑器错误边界（捕获 Slate DOM 同步错误，静默重建编辑器）
import SlateErrorBoundary from './slate/SlateErrorBoundary';
// Slate Mention 元素组件（VibeCardMention / FileMentionElement / Element / Leaf）
import {
    VibeCardMention,
    FileMentionElement,
    Element,
    Leaf,
} from './slate/MentionElements';
// Slate 插件函数 + CustomEditor / VibeCardData 类型
import {
    withVibeCardMentions,
    insertVibeCardMention,
    type CustomEditor,
    type VibeCardData,
} from './slate/slatePlugins';
// 输入历史 hook（localStorage 持久化 + 键盘导航 + Modal UI 状态）
import { useInputHistory } from '../hooks/slate/useInputHistory';
// 附件管理 hook（消除 paste/drop 内部 verbatim 重复；导出 AttachmentFile 类型）
import { useAttachments, type AttachmentFile } from '../hooks/slate/useAttachments';

// ===== Slate 类型定义 =====
// CustomEditor / VibeCardData 已迁移至 ./slate/slatePlugins.ts

// 斜杠命令类型
interface SlashCommand {
    name: string;
    description: string;
}

// Props 类型定义
interface SlateInputWithSenderProps {
    onSubmit: (
        text: string,
        vibeCardRefs: Array<{ id: string; name: string }>,
        attachments: AttachmentFile[],
        fileRefs?: Array<{ path: string; name: string }>,
    ) => void;
    loading: boolean;
    onVibeCardInsert?: (insertFn: (data: VibeCardData) => void) => void;
    onModelChange?: (model: ModelSelection) => void;
    currentModel?: ModelSelection;
    visionCapable?: boolean;
    customConfigs?: CustomModelConfig[];
    onSaveConfigs?: (configs: CustomModelConfig[]) => void;
    selectedConfigId?: string | null;
    onInsertTextReady?: (insertFn: (text: string) => void) => void;
    threadContextInjection?: boolean;
    onThreadContextInjectionToggle?: (enabled: boolean) => void;
    slashCommands?: SlashCommand[];
    placeholder?: string;
}

/**
 * SlateInputWithSender 主组件
 * JayRead AI Chat 的富文本输入框组件，集成了编辑器、工具栏、模型选择和自定义模型管理功能
 * 样式参考 Ant Design X 的 Sender 组件，但使用 Slate.js 作为底层编辑器
 *
 * @param {Object} props - 组件属性
 * @param {Function} props.onSubmit - 提交消息的回调函数，参数为 (text, vibeCardRefs, images)
 * @param {boolean} props.loading - 是否正在发送中（由父组件控制，用于禁用发送按钮和编辑器）
 * @param {Function} props.onVibeCardInsert - 注册 VibeCard 插入方法的回调，参数为插入函数
 * @param {Function} props.onModelChange - 模型切换的回调函数，参数为新的模型对象
 * @param {Object} props.currentModel - 当前选中的模型信息，包含 key、label、configId 等字段
 * @param {boolean} [props.visionCapable=true] - 当前模型是否支持图片/多模态输入
 * @param {Array} [props.customConfigs=[]] - 自定义模型配置列表（由父组件管理）
 * @param {Function} props.onSaveConfigs - 保存配置的回调函数，参数为 (configs) 配置数组
 * @param {string} [props.selectedConfigId] - 当前选中的配置 ID（由父组件管理）
 * @param {Function} [props.onInsertTextReady] - 注册文本插入方法的回调，参数为插入函数
 *   父组件可调用该函数将文本插入到 Slate 编辑器中（用于上下文注入预填充输入框）
 */
const SlateInputWithSender = forwardRef<unknown, SlateInputWithSenderProps>(({
    onSubmit,              // 消息提交回调
    loading,               // 加载状态
    onVibeCardInsert,      // VibeCard 插入回调
    onModelChange,         // 模型切换回调
    currentModel,          // 当前模型
    visionCapable = true,  // 是否支持多模态（图片），默认为 true
    customConfigs = [],    // 自定义模型配置列表（由父组件传入）
    onSaveConfigs,         // 保存配置回调（由父组件传入）
    selectedConfigId,      // 当前选中的配置 ID（由父组件传入）
    onInsertTextReady,     // 文本插入方法注册回调（由父组件传入）
    threadContextInjection = false,  // 线程上下文注入开关状态
    onThreadContextInjectionToggle,  // 线程上下文注入开关切换回调
    slashCommands = [],              // 斜杠命令列表（由父组件传入，用于自动补全提示）
    placeholder = 'Enter 发送, Shift+Enter 换行', // 输入框占位提示文本
}, ref) => {
    const { message: antMessage } = App.useApp();
    // 获取 Ant Design 的主题 token（包含颜色、字号等设计变量）
    const { token } = theme.useToken();

    // editorKey: 用于强制重建 editor 实例的版本号。
    // 当 SlateErrorBoundary 捕获到 DOM 同步错误时递增，触发新 editor 的创建，
    // 彻底清除旧实例中过期的 KEY_TO_ELEMENT 映射。
    const [editorKey, setEditorKey] = useState(0);
    const editorRef = useRef<CustomEditor>();
    const prevKeyRef = useRef(0);

    // 延迟初始化或重建编辑器：editorKey 变化时创建新实例
    if (!editorRef.current || prevKeyRef.current !== editorKey) {
        editorRef.current = withVibeCardMentions(withReact(withHistory(createEditor())));
        prevKeyRef.current = editorKey;
    }
    const editor = editorRef.current!;

    // 错误边界回调：重建 editor 实例
    const handleSlateError = useCallback(() => {
        setEditorKey(k => k + 1);
    }, []);

    // 跟踪组件是否已挂载，防止卸载后 Slate 的内部事件处理尝试访问已移除的 DOM
    const mountedRef = useRef(true);

    // 组件卸载时清除编辑器选区并标记为已卸载，防止 Slate 在 DOM 已销毁后仍尝试解析节点
    useEffect(() => {
        mountedRef.current = true;
        return () => {
            mountedRef.current = false;
            try {
                const ed = editorRef.current;
                if (ed) {
                    Transforms.deselect(ed);
                    ReactEditor.blur(ed);
                }
            } catch (_) {}
        };
    }, [editorKey]);

    // 编辑器的值状态：Slate 的内容模型是一个节点树（JSON 数组）
    // 初始值为一个空的段落节点
    const [value, setValue] = useState<Descendant[]>([
        {
            type: 'paragraph' as const,       // 段落类型
            children: [{ text: '' }], // 段落内包含一个空文本节点
        } as Descendant,
    ]);

    // ===== 斜杠命令自动补全相关状态 =====
    // slashMenuVisible：是否显示斜杠命令补全菜单
    const [slashMenuVisible, setSlashMenuVisible] = useState(false);
    // slashFilter：当前输入的斜杠命令过滤文本（如输入 /ne 时为 'ne'）
    const [slashFilter, setSlashFilter] = useState('');
    // slashMenuIndex：当前高亮的菜单项索引（用于键盘上下选择）
    const [slashMenuIndex, setSlashMenuIndex] = useState(0);

    // 根据当前过滤文本计算匹配的命令列表
    const filteredCommands = useMemo(() => {
        if (!slashFilter) return slashCommands;
        return slashCommands.filter((cmd: SlashCommand) =>
            cmd.name.startsWith('/' + slashFilter.toLowerCase())
        );
    }, [slashCommands, slashFilter]);

    // 斜杠命令起始位置 ref（记录 '/' 在编辑器中的偏移量，用于 Tab 补全时替换文本）
    const slashStartOffsetRef = useRef<number | null>(null);
    // 斜杠命令菜单容器 ref，用于键盘导航时 scrollIntoView
    const slashMenuListRef = useRef<HTMLDivElement>(null);

    // 键盘导航时自动滚动到高亮项
    useEffect(() => {
        if (!slashMenuListRef.current || filteredCommands.length === 0) return;
        const activeItem = slashMenuListRef.current.children[slashMenuIndex] as HTMLElement;
        if (activeItem) {
            activeItem.scrollIntoView({ block: 'nearest' });
        }
    }, [slashMenuIndex, filteredCommands.length]);

    // ===== 输入历史管理 =====
    // 历史 ref / Modal UI 状态 / extractContent / setEditorText / saveToHistory / navigateHistory
    // 已迁移到 useInputHistory（封装 localStorage + 键盘导航 + Modal UI）
    const {
        inputHistoryRef,
        historyIndexRef,
        isNavigatingRef,
        extractContent,
        setEditorText,
        historyModalOpen,
        historySearchFilter,
        historyListForModal,
        historyHoverIndex,
        saveToHistory,
        navigateHistory,
        handleHistorySelect,
        resetHistoryIndex,
        openHistoryModal,
        closeHistoryModal,
        setHistorySearchFilter,
        setHistoryHoverIndex,
    } = useInputHistory({ editor, setValue });

    // ===== 附件管理（图片/文本统一）=====
    // attachedFiles / isDragging / dragCounterRef / handleFileSelect / handlePaste / handleFileDrop
    // 已迁移到 useAttachments（消除原 handlePaste/handleDrop 内部 verbatim 重复）
    const {
        attachedFiles,
        setAttachedFiles,
        isDragging,
        setIsDragging,
        dragCounterRef,
        handleFileSelect,
        handleAttachmentRemove,
        handlePaste,
        handleFileDrop,
    } = useAttachments({ visionCapable, currentModel });

    // 元素渲染器回调（稳定引用，不会因重渲染而变化）
    const renderElement = useCallback((props: RenderElementProps) => <Element {...props as any} />, []);
    // 叶子节点渲染器回调（稳定引用）
    const renderLeaf = useCallback((props: RenderLeafProps) => <Leaf {...props as any} />, []);

    /**
     * 向父组件暴露 VibeCard 插入方法
     * 通过 onVibeCardInsert 回调，将 insertVibeCardMention 函数注册给父组件
     * 父组件可以在外部（如侧边栏的文献列表）触发 VibeCard 的插入操作
     */
    useEffect(() => {
        if (onVibeCardInsert) {
            // 传递一个闭包函数给父组件，该闭包捕获了当前编辑器实例
            onVibeCardInsert((vibeCardData: VibeCardData) => {
                if (!mountedRef.current) return;
                try {
                    insertVibeCardMention(editor, vibeCardData);
                } catch (_) {}
            });
        }
    }, [editor, onVibeCardInsert]);

    /**
     * 向父组件暴露文本插入方法
     * 通过 onInsertTextReady 回调，将 insertText 函数注册给父组件
     * 父组件可以调用该函数将指定文本插入到编辑器光标位置（用于上下文注入预填充输入框）
     *
     * 插入行为：
     * - 如果编辑器当前有内容，在现有内容末尾追加一个换行后再插入新文本
     * - 如果编辑器为空，直接插入文本
     * - 插入完成后自动将光标移动到文本末尾，并聚焦编辑器
     */
    useEffect(() => {
        if (onInsertTextReady) {
            onInsertTextReady((text: string) => {
                // 防止组件已卸载时操作编辑器 DOM
                if (!mountedRef.current) return;
                try {
                    // 先将光标移到编辑器末尾
                    Transforms.select(editor, Editor.end(editor, []));
                    // 如果编辑器已有内容，先插入换行分隔
                    if (!isEditorEmpty()) {
                        Transforms.insertText(editor, '\n');
                    }
                    // 插入目标文本
                    Transforms.insertText(editor, text);
                    // 聚焦编辑器，让用户可以直接编辑或发送
                    ReactEditor.focus(editor);
                } catch (_) {}
            });
        }
    }, [editor, onInsertTextReady]);

    /**
     * 检查编辑器是否为空
     * 综合判断三个维度：文本内容、VibeCard 引用、附件列表
     * 仅当三者全部为空时才认为编辑器为空
     */
    const isEditorEmpty = () => {
        const { text, vibeCardRefs } = extractContent();
        return !text.trim() && vibeCardRefs.length === 0 && attachedFiles.length === 0;
    };

    /** 获取光标所在行号和总行数 */
    const getCursorLineInfo = (): { lineIndex: number; totalLines: number } => {
        try {
            const { selection } = editor;
            if (!selection) return { lineIndex: 0, totalLines: 1 };
            const totalLines = editor.children.length;
            const currentBlock = Editor.above(editor, {
                match: (n: Node) => Editor.isBlock(editor, n as Parameters<typeof Editor.isBlock>[1]),
            });
            if (!currentBlock) return { lineIndex: 0, totalLines };
            const lineIndex = currentBlock[1][0] as number;
            return { lineIndex, totalLines };
        } catch {
            return { lineIndex: 0, totalLines: 1 };
        }
    };

    // 暴露方法给父组件：配置弹窗、文件上传、文本插入
    useImperativeHandle(ref, () => ({
        openConfigModal: () => {
                void NiceModal.show('model-config', {
                    customConfigs,
                    onSaveConfigs,
                    selectedConfigId,
                    onModelChange,
                    threadContextInjection,
                    onThreadContextInjectionToggle,
                });
            },
        handleFileSelect,
        focus: () => {
            // 聚焦 Slate 编辑器
            ReactEditor.focus(editor);
        },
        insertText: (text: string) => {
            // 在编辑器中插入文本（用于快捷操作等场景）
            if (!mountedRef.current) return;
            try {
                Transforms.select(editor, Editor.end(editor, []));
                if (!isEditorEmpty()) {
                    Transforms.insertText(editor, '\n');
                }
                Transforms.insertText(editor, text);
                ReactEditor.focus(editor);
            } catch (_) {}
        },
    }), [handleFileSelect, editor]);

    // 内部提交状态锁，用于防止快速重复点击导致的竞态问题
    // 使用 useRef 而非 useState，因为 ref 的更新是同步的（立即生效），
    // 可以在同一个事件循环中立即阻止第二次回车提交
    // 而 useState 的更新是异步的（batched），在快速连续回车时可能来不及阻止第二次提交
    const submittingRef = useRef(false);

    /**
     * 处理消息发送
     * 验证输入内容后提取文本、VibeCard 引用和附件列表，调用父组件的 onSubmit
     * 发送完成后立即清空编辑器和附件列表，提升用户体验
     */
    const handleSubmit = useCallback(async () => {
        // 防止重复提交的三重检查：
        // 1. isEditorEmpty(): 内容为空
        // 2. loading: 父组件的异步加载状态（如网络请求中）
        // 3. submittingRef.current: 本地的同步锁（防止快速连续回车）
        if (isEditorEmpty() || loading || submittingRef.current) return;

        // 立即设置本地提交锁（同步更新，立即生效，可阻止后续的快速回车）
        submittingRef.current = true;

        try {
            // 从编辑器中提取纯文本内容和 VibeCard 引用列表
            const { text, vibeCardRefs, fileRefs } = extractContent();

            // 在清空之前保存当前附件数组的副本（避免引用被清空操作影响）
            const filesToSend = [...attachedFiles];

            // 立即清空编辑器内容和附件列表（提升用户体验，不等待 onSubmit 的异步结果）
            // 通过选中编辑器全部内容并删除来实现清空
            Transforms.delete(editor, {
                at: {
                    anchor: Editor.start(editor, []), // 编辑器内容的起始位置
                    focus: Editor.end(editor, []),     // 编辑器内容的结束位置
                },
            });
            setAttachedFiles([]); // 清空附件列表

            // 调用父组件的 onSubmit 回调，传递消息内容
            // 父组件会设置 loading 状态并进行后续处理
            // 注意：这里不等待 onSubmit 的结果，让父组件自行处理后续逻辑
            // 第 3 个参数改为 attachments（统一的附件数组，包含图片/文本/二进制文件）
            saveToHistory(text);
            resetHistoryIndex();
            onSubmit(text, vibeCardRefs, filesToSend, fileRefs);

        } catch (e) {
            console.error('[SlateInput] handleSubmit execution failed:', e);
            antMessage.error('发送失败: ' + (e as Error).message);
        } finally {
            // 无论成功或失败，都释放提交锁
            submittingRef.current = false;
        }
    }, [loading, attachedFiles, editor, onSubmit]);

    /**
     * 处理键盘事件
     * 监听编辑器中的按键操作：
     * - Enter：提交消息（在禁用状态下忽略）
     * - Shift+Enter：Slate 默认行为，插入换行
     * - Tab：补全斜杠命令（当菜单可见时）
     * - Escape：关闭斜杠命令菜单
     * - 上/下方向键：在斜杠命令菜单中导航
     *
     * @param {React.KeyboardEvent} event - 键盘事件对象
     */
    const handleKeyDown = useCallback(
        (event: React.KeyboardEvent) => {
            // ===== 输入历史快捷键（最高优先级） =====
            // Ctrl+R: 显示历史记录弹窗
            if (event.key === 'r' && event.ctrlKey && !event.altKey && !event.metaKey) {
                event.preventDefault();
                openHistoryModal();
                return;
            }
            // Ctrl+Alt+Up/Down: 无条件导航历史（不管光标位置）
            if (event.ctrlKey && event.altKey && !event.metaKey) {
                if (event.key === 'ArrowUp') {
                    event.preventDefault();
                    navigateHistory('up');
                    return;
                }
                if (event.key === 'ArrowDown') {
                    event.preventDefault();
                    navigateHistory('down');
                    return;
                }
            }

            // ===== 斜杠命令菜单键盘交互 =====
            if (slashMenuVisible && filteredCommands.length > 0) {
                if (event.key === 'Tab') {
                    // Tab 键：补全第一个（或当前高亮的）匹配命令并直接执行
                    event.preventDefault();
                    const targetCmd = filteredCommands[slashMenuIndex] || filteredCommands[0];
                    if (targetCmd) {
                        // 关闭菜单
                        setSlashMenuVisible(false);
                        setSlashFilter('');
                        slashStartOffsetRef.current = null;
                        // 清空编辑器并发送命令
                        Transforms.delete(editor, {
                            at: {
                                anchor: Editor.start(editor, []),
                                focus: Editor.end(editor, []),
                            },
                        });
                        onSubmit(targetCmd.name, [], []);
                    }
                    return;
                }
                if (event.key === 'Escape') {
                    // Escape：关闭菜单
                    event.preventDefault();
                    setSlashMenuVisible(false);
                    return;
                }
                if (event.key === 'ArrowDown') {
                    // 下方向键：向下移动高亮
                    event.preventDefault();
                    setSlashMenuIndex(prev =>
                        prev < filteredCommands.length - 1 ? prev + 1 : 0
                    );
                    return;
                }
                if (event.key === 'ArrowUp') {
                    // 上方向键：向上移动高亮
                    event.preventDefault();
                    setSlashMenuIndex(prev =>
                        prev > 0 ? prev - 1 : filteredCommands.length - 1
                    );
                    return;
                }
                if (event.key === 'Enter' && !event.shiftKey) {
                    // Enter 键：选中命令后直接执行，无需再次按回车
                    event.preventDefault();
                    const targetCmd = filteredCommands[slashMenuIndex] || filteredCommands[0];
                    if (targetCmd) {
                        setSlashMenuVisible(false);
                        setSlashFilter('');
                        slashStartOffsetRef.current = null;
                        Transforms.delete(editor, {
                            at: {
                                anchor: Editor.start(editor, []),
                                focus: Editor.end(editor, []),
                            },
                        });
                        onSubmit(targetCmd.name, [], []);
                    }
                    return;
                }
            }

            // ===== Up/Down 方向键历史导航 =====
            if (!slashMenuVisible && !event.ctrlKey && !event.altKey && !event.metaKey) {
                if (event.key === 'ArrowUp') {
                    const { lineIndex } = getCursorLineInfo();
                    if (lineIndex === 0) {
                        event.preventDefault();
                        navigateHistory('up');
                    }
                }
                if (event.key === 'ArrowDown') {
                    const { lineIndex, totalLines } = getCursorLineInfo();
                    if (lineIndex === totalLines - 1) {
                        event.preventDefault();
                        navigateHistory('down');
                    }
                }
            }

            // 仅处理 Enter 键（Shift+Enter 由 Slate 默认处理为换行）
            if (event.key === 'Enter' && !event.shiftKey) {
                // 阻止 Enter 的默认行为（防止插入换行符）
                event.preventDefault();
                // 禁用状态检查：
                // - loading: 父组件正在处理中
                // - submittingRef.current: 本地提交锁（防止快速连续回车）
                // - isEditorEmpty(): 编辑器内容为空
                if (loading || submittingRef.current || isEditorEmpty()) {
                    return;
                }
                // 执行提交
                handleSubmit();
            }
        },
        [loading, attachedFiles, handleSubmit, slashMenuVisible, filteredCommands, slashMenuIndex, editor]
    );

    /**
     * 处理拖拽放置事件（Drag & Drop）
     * 检测是否是从外部拖入的 VibeCard 引用
     * 如果是，则解析数据并插入 VibeCard mention 到编辑器
     *
     * @param {React.DragEvent} event - 拖拽放置事件
     */
    const handleDrop = useCallback(
        async (event: React.DragEvent) => {
            // 重置拖拽视觉反馈状态
            dragCounterRef.current = 0;
            setIsDragging(false);

            const dt = event.dataTransfer;

            // 1) VibeCard 拖拽：自定义 MIME，需要 editor 才能插入 mention（保留在此处）
            const vibeCardData = dt.getData('application/x-jayread-vibecard-reference');
            if (vibeCardData) {
                event.preventDefault();
                event.stopPropagation();
                try {
                    const data = JSON.parse(vibeCardData);
                    if (data.vibeCardId) {
                        insertVibeCardMention(editor, {
                            id: data.vibeCardId,
                            name: data.vibeCardName || data.vibeCardId,
                            type: data.type,
                            vibeCardType: data.vibeCardType,
                        });
                    }
                } catch (error) {
                    console.error('[SlateInput] Error parsing VibeCard drop data:', error);
                }
                return;
            }

            // 2) 文件拖拽：收集 File 列表后委托给 handleFileDrop（hook 内统一分类/读取）
            const droppedFiles: File[] = [];
            if (dt.files?.length) {
                for (let i = 0; i < dt.files.length; i++) {
                    droppedFiles.push(dt.files[i]);
                }
            }
            if (droppedFiles.length > 0) {
                event.preventDefault();
                event.stopPropagation();
                await handleFileDrop(droppedFiles);
            }
            // 既不是 VibeCard 也没有文件 → 不阻止默认（纯文本拖入由浏览器处理）
        },
        [editor, handleFileDrop]
    );

    /**
     * 处理拖拽悬停事件（Drag Over）
     * 当用户将文件或 VibeCard 拖入编辑器区域上方时会持续触发此事件。
     * 浏览器默认不允许在可编辑区域放置外部文件，因此必须在此处调用
     * preventDefault() 来明确告知浏览器"允许放置"，否则后续的 drop
     * 事件不会触发。同时设置 dropEffect 为 'copy' 以显示复制光标。
     *
     * 支持 VibeCard 引用和图片文件两种拖拽来源。
     *
     * @param {React.DragEvent} event - 拖拽悬停事件
     */
    const handleDragOver = useCallback(
        (event: React.DragEvent) => {
            // 获取 DataTransfer 对象，包含正在拖拽的数据的类型信息
            const dt = event.dataTransfer;
            // types 是一个 DOMStringList，包含当前拖拽中可用的数据类型
            const types = dt.types;

            // 检查是否为 VibeCard 引用拖拽（自定义 MIME 类型）
            if (types.includes('application/x-jayread-vibecard-reference')) {
                // 必须调用 preventDefault，否则浏览器不会允许放置操作
                event.preventDefault();
                // 阻止事件冒泡，防止父元素的 dragover 处理器干扰
                event.stopPropagation();
                // 设置拖拽效果为"复制"（显示带 + 号的复制光标）
                dt.dropEffect = 'copy';
                return;
            }

            // 检查是否为文件拖拽（从文件管理器等外部来源拖入的文件）
            if (types.includes('Files')) {
                // 同样需要 preventDefault 来允许放置
                event.preventDefault();
                event.stopPropagation();
                dt.dropEffect = 'copy';
            }
            // 如果既不是 VibeCard 也不是文件拖拽，则不干预，
            // 让浏览器使用默认行为（如编辑器内文本的拖拽移动）
        },
        []
    );

    /**
     * 处理拖拽进入事件（Drag Enter）
     * 当文件或 VibeCard 拖入编辑器容器区域时触发
     * 使用计数器追踪嵌套的 dragEnter/dragLeave 事件
     * （拖入子元素时会额外触发父元素的 leave 和子元素的 enter）
     */
    const handleDragEnter = useCallback(
        (event: React.DragEvent) => {
            const dt = event.dataTransfer;
            const types = dt.types;

            // 只响应文件拖拽或 VibeCard 拖拽
            if (types.includes('Files') || types.includes('application/x-jayread-vibecard-reference')) {
                event.preventDefault();
                dragCounterRef.current++;
                setIsDragging(true);
            }
        },
        []
    );

    /**
     * 处理拖拽离开事件（Drag Leave）
     * 当文件或 VibeCard 离开编辑器容器区域时触发
     * 使用计数器确保只有真正离开容器时才取消视觉反馈
     */
    const handleDragLeave = useCallback(
        (event: React.DragEvent) => {
            event.preventDefault();
            dragCounterRef.current--;
            if (dragCounterRef.current <= 0) {
                dragCounterRef.current = 0;
                setIsDragging(false);
            }
        },
        []
    );

    /**
     * 处理编辑器内容变化事件
     * 当用户在编辑器中输入、删除或修改内容时触发
     * 更新 Slate 的值状态，同时检测斜杠命令输入
     *
     * 斜杠命令检测逻辑：
     * - 当编辑器内容以 '/' 开头时，提取 '/' 后面的文本作为过滤条件
     * - 如果 '/' 不在行首或编辑器有多个非空段落，则不触发补全
     *
     * @param {Array} newValue - Slate 的新值（节点树）
     */
    const handleChange = useCallback(
        (newValue: Descendant[]) => {
            // 用户输入时重置历史导航状态
            if (!isNavigatingRef.current) {
                resetHistoryIndex();
            }
            setValue(newValue);

            // ===== 斜杠命令检测 =====
            // 只在编辑器第一个段落的文本以 '/' 开头时触发
            const firstNode = newValue[0];
            if (firstNode && (firstNode as { children?: unknown }).children) {
                const text = ((firstNode as { children: Node[] }).children).map((c: Node) => (c as { text?: string }).text || '').join('');
                if (text.startsWith('/') && text.length <= 20 && !text.includes(' ')) {
                    // 提取 '/' 后面的部分作为过滤文本
                    const filter = text.slice(1);
                    setSlashFilter(filter);
                    setSlashMenuVisible(true);
                    setSlashMenuIndex(0);
                    // 记录 '/' 的起始偏移量
                    slashStartOffsetRef.current = 0;
                } else {
                    setSlashMenuVisible(false);
                    setSlashFilter('');
                    slashStartOffsetRef.current = null;
                }
            } else {
                setSlashMenuVisible(false);
                setSlashFilter('');
                slashStartOffsetRef.current = null;
            }
        },
        []
    );


    // ===== JSX 渲染 =====
    return (
        <div style={{ position: 'relative' }}>
        <div
            style={{
                background: isDragging
                    ? `${token.colorPrimary}08`
                    : 'transparent',
                border: isDragging
                    ? `2px dashed ${token.colorPrimary}`
                    : 'none',
                borderRadius: isDragging ? '8px' : '0',
                transition: 'background-color 0.2s',
                boxShadow: isDragging
                    ? `0 0 0 2px ${token.colorPrimary}25`
                    : 'none',
            }}
            className="slate-sender-container" // CSS 类名（用于外部样式覆盖）
            onDragEnter={handleDragEnter}
            onDragLeave={handleDragLeave}
            onDrop={() => {
                // 拖拽放置时重置视觉反馈（处理 drop 在 AttachmentPreview 等非 Editable 区域的情况）
                dragCounterRef.current = 0;
                setIsDragging(false);
            }}
        >
            {/* 附件预览区域：显示在编辑器上方，展示待发送的图片缩略图和文件卡片 */}
            <AttachmentPreview
                attachments={attachedFiles}     // 统一的附件列表（图片+文本+二进制）
                onRemove={handleAttachmentRemove} // 移除附件的回调
            />

            {/* Slate 编辑器区域 */}
            <div style={{ padding: 0 }}>
                {/* 错误边界：捕获 Slate DOM 解析错误，重建 editor 实例并恢复 */}
                <SlateErrorBoundary onSlateError={handleSlateError}>
                {/* key={editorKey} 强制 Slate 完全重新挂载，清除过期的 DOM 映射 */}
                <Slate key={editorKey} editor={editor} initialValue={value} onChange={handleChange}>
                    {/* Editable 可编辑区域：Slate 的核心渲染组件 */}
                    <Editable
                        renderElement={renderElement}   // 自定义元素渲染器
                        renderLeaf={renderLeaf}         // 自定义叶子节点渲染器
                        onKeyDown={handleKeyDown}       // 键盘事件处理
                        onPaste={handlePaste}           // 粘贴事件处理（拦截图片粘贴）
                        onDrop={handleDrop}             // 拖拽放置事件处理（VibeCard 引用）
                        onDragOver={handleDragOver}     // 拖拽悬停事件处理
                        placeholder={placeholder} // 空内容时的占位提示
                        disabled={loading}              // 加载中时禁用编辑
                        style={{
                            minHeight: '36px',          // 最小高度
                            maxHeight: '120px',         // 最大高度（超出后出现滚动条）
                            fontSize: '14px',           // 正文字号
                            color: 'var(--text-primary, #262626)', // 文字颜色：使用全局主文字颜色变量
                            outline: 'none',            // 移除默认聚焦轮廓
                            overflowY: 'auto',          // 垂直方向内容溢出时显示滚动条
                            lineHeight: '1.5',          // 行高
                        }}
                    />
                </Slate>
                </SlateErrorBoundary>
            </div>


            {/* 自定义模型配置弹窗（通过 NiceModal 命令式调用） */}
        </div>

        {/* 斜杠命令自动补全菜单（渲染在 slate-sender-container 外部，避免被 overflow: hidden 裁切） */}
        {slashMenuVisible && filteredCommands.length > 0 && (
            <div className="slash-command-menu" ref={slashMenuListRef}>
                {filteredCommands.map((cmd: SlashCommand, idx: number) => (
                    <div
                        key={cmd.name}
                        className={`slash-command-item${idx === slashMenuIndex ? ' active' : ''}`}
                        onMouseDown={(e: React.MouseEvent) => {
                            // 使用 mouseDown 而非 click，避免编辑器失焦
                            e.preventDefault();
                            setSlashMenuVisible(false);
                            setSlashFilter('');
                            slashStartOffsetRef.current = null;
                            Transforms.delete(editor, {
                                at: {
                                    anchor: Editor.start(editor, []),
                                    focus: Editor.end(editor, []),
                                },
                            });
                            onSubmit(cmd.name, [], []);
                        }}
                    >
                        <span className="slash-command-name">{cmd.name}</span>
                        <span className="slash-command-desc">{cmd.description}</span>
                    </div>
                ))}
            </div>
        )}

        {/* 输入历史弹窗（Ctrl+R 触发） */}
        <Modal
            title="输入历史"
            open={historyModalOpen}
            onCancel={() => closeHistoryModal()}
            footer={null}
            width={640}
            styles={{ body: { padding: 0 } }}
            destroyOnHidden
        >
            <div style={{ padding: '12px 16px', borderBottom: '1px solid var(--border-color, #f0f0f0)' }}>
                <Input
                    placeholder="搜索历史记录..."
                    value={historySearchFilter}
                    onChange={e => { setHistorySearchFilter(e.target.value); setHistoryHoverIndex(-1); }}
                    allowClear
                    autoFocus
                />
            </div>
            <div
                style={{ maxHeight: '50vh', overflowY: 'auto' }}
                onMouseLeave={() => setHistoryHoverIndex(-1)}
            >
                {historyListForModal
                    .filter(item => !historySearchFilter || item.toLowerCase().includes(historySearchFilter.toLowerCase()))
                    .length === 0 ? (
                    <div style={{ padding: '32px 16px', textAlign: 'center', color: 'var(--text-tertiary, #999)' }}>
                        {historyListForModal.length === 0 ? '暂无历史记录' : '没有匹配的记录'}
                    </div>
                ) : (
                    historyListForModal
                        .filter(item => !historySearchFilter || item.toLowerCase().includes(historySearchFilter.toLowerCase()))
                        .map((item, idx) => (
                            <div
                                key={idx}
                                style={{
                                    padding: '8px 16px',
                                    cursor: 'pointer',
                                    borderBottom: '1px solid var(--border-color, #f0f0f0)',
                                    backgroundColor: historyHoverIndex === idx
                                        ? 'var(--bg-secondary, #f5f5f5)'
                                        : 'transparent',
                                    transition: 'background-color 0.15s',
                                    whiteSpace: 'pre-wrap',
                                    wordBreak: 'break-word',
                                    fontSize: '13px',
                                    lineHeight: '1.5',
                                    maxHeight: '80px',
                                    overflow: 'hidden',
                                    color: 'var(--text-primary, #262626)',
                                }}
                                onMouseEnter={() => setHistoryHoverIndex(idx)}
                                onClick={() => handleHistorySelect(item)}
                            >
                                {item}
                            </div>
                        ))
                )}
            </div>
        </Modal>
        </div>
    );
});

// 默认导出 SlateInputWithSender 组件
export default SlateInputWithSender;
