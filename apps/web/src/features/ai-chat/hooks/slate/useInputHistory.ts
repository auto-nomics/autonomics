/**
 * 输入历史 Hook（↑↓ 导航 + Ctrl+R 弹窗）
 *
 * 管理 Slate 编辑器输入历史的完整生命周期：
 * - localStorage 持久化（key: 'jayread-input-history'，最多 300 条）
 * - ↑/↓ 键盘导航：在第一行按 ↑ 回到上一条历史，在最后一行按 ↓ 前进
 * - Ctrl+R 弹窗：模糊搜索 + 鼠标 hover + 点击选中
 *
 * 与编辑器的耦合：
 * - 接收 `editor` + `setValue` 作为参数
 * - 内部封装 extractContent（读编辑器文本+引用）/ setEditorText（替换编辑器内容）
 * - 这些工具方法对外暴露（handleSubmit / isEditorEmpty 也需要用）
 *
 * 抽出原因：原 SlateInputWithSender 内历史管理散布在 useState×4 + useRef×4 + 5 个函数，
 * 独立成 hook 让 SlateInputWithSender 专注于消息构建/发送主流程。
 *
 * @module ai-chat/hooks/slate/useInputHistory
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { Editor, Node, Transforms, type Descendant } from 'slate';
import { ReactEditor } from 'slate-react';
import type { CustomEditor } from '../../components/slate/slatePlugins';
import type { VibeCardMentionElement } from '../../components/slate/MentionElements';
import type { FileMentionElement as FileMentionElementType } from '../../components/slate/MentionElements';

/** 历史记录最大条数（超出后丢弃最旧的） */
const MAX_HISTORY = 300;
/** localStorage 持久化 key */
const HISTORY_STORAGE_KEY = 'jayread-input-history';

/** useInputHistory 参数 */
interface UseInputHistoryParams {
    /** Slate 编辑器实例 */
    editor: CustomEditor;
    /** Slate 值的 state setter（setEditorText 替换内容时同步更新） */
    setValue: React.Dispatch<React.SetStateAction<Descendant[]>>;
}

/** 编辑器内容快照（文本 + VibeCard 引用 + 文件引用） */
export interface EditorContent {
    text: string;
    vibeCardRefs: Array<{ id: string; name: string }>;
    fileRefs: Array<{ path: string; name: string }>;
}

/**
 * 输入历史 Hook
 *
 * @returns inputHistoryRef/historyIndexRef/isNavigatingRef/draftRef 内部状态镜像，extractContent/setEditorText 编辑器工具，historyModalOpen/historySearchFilter/historyListForModal/historyHoverIndex Modal UI 状态，saveToHistory/navigateHistory/handleHistorySelect/resetHistoryIndex/openHistoryModal/closeHistoryModal/setHistorySearchFilter/setHistoryHoverIndex 动作
 */
export function useInputHistory({ editor, setValue }: UseInputHistoryParams) {
    // ===== ref-based 状态 =====
    const inputHistoryRef = useRef<string[]>([]);
    const historyIndexRef = useRef(-1); // -1 表示未在导航历史
    const isNavigatingRef = useRef(false); // setEditorText 期间为 true，防止 onChange 重置 historyIndexRef
    const draftRef = useRef(''); // 用户当前草稿（开始导航历史前的内容），回到 -1 时恢复

    // ===== Modal UI 状态 =====
    const [historyModalOpen, setHistoryModalOpen] = useState(false);
    const [historySearchFilter, setHistorySearchFilter] = useState('');
    const [historyListForModal, setHistoryListForModal] = useState<string[]>([]);
    const [historyHoverIndex, setHistoryHoverIndex] = useState(-1);

    // 从 localStorage 加载历史记录（仅挂载时一次）
    useEffect(() => {
        try {
            const stored = localStorage.getItem(HISTORY_STORAGE_KEY);
            if (stored) {
                inputHistoryRef.current = JSON.parse(stored);
            }
        } catch {}
    }, []);

    /** 提取编辑器当前内容：文本 + VibeCard 引用 + 文件引用 + Citation 引用 */
    const extractContent = useCallback((): EditorContent => {
        const text = editor.children
            .map((node: Node) => {
                return (node as { children: Node[] }).children
                    .map((child: Node) => {
                        if ((child as VibeCardMentionElement).type === 'vibecard-mention') {
                            return `@${(child as VibeCardMentionElement).vibeCardName || (child as VibeCardMentionElement).vibeCardId}`;
                        }
                        if ((child as FileMentionElementType).type === 'file-mention') {
                            return `@${(child as FileMentionElementType).filePath}`;
                        }
                        return (child as { text?: string }).text || '';
                    })
                    .join('');
            })
            .join('\n');

        const vibeCardRefs: Array<{ id: string; name: string }> = [];
        const fileRefs: Array<{ path: string; name: string }> = [];
        editor.children.forEach((node: Node) => {
            (node as { children: Node[] }).children.forEach((child: Node) => {
                if ((child as VibeCardMentionElement).type === 'vibecard-mention') {
                    vibeCardRefs.push({
                        id: (child as VibeCardMentionElement).vibeCardId,
                        name: (child as VibeCardMentionElement).vibeCardName || (child as VibeCardMentionElement).vibeCardId,
                    });
                }
                if ((child as FileMentionElementType).type === 'file-mention') {
                    fileRefs.push({ path: (child as FileMentionElementType).filePath, name: (child as FileMentionElementType).fileName });
                }
            });
        });

        return { text, vibeCardRefs, fileRefs };
    }, [editor]);

    /** 替换编辑器内容为指定文本（导航历史时使用） */
    const setEditorText = useCallback((text: string) => {
        isNavigatingRef.current = true;
        try {
            const lines = text ? text.split('\n') : [''];
            const nodes = lines.map(line => ({
                type: 'paragraph' as const,
                children: [{ text: line }],
            }));
            editor.children = nodes;
            Editor.normalize(editor, { force: true });
            setValue(nodes);
            Transforms.select(editor, Editor.end(editor, []));
        } catch {}
        // 延迟重置：确保 React 重渲染期间 Slate 触发的异步 onChange
        // 不会因 isNavigatingRef 已为 false 而误将 historyIndexRef 重置为 -1
        setTimeout(() => {
            isNavigatingRef.current = false;
        }, 0);
    }, [editor, setValue]);

    /** 将用户输入保存到历史记录 */
    const saveToHistory = useCallback((text: string) => {
        if (!text || !text.trim()) return;
        const history = inputHistoryRef.current;
        const filtered = history.filter(item => item !== text);
        const newHistory = [text, ...filtered].slice(0, MAX_HISTORY);
        inputHistoryRef.current = newHistory;
        try {
            localStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify(newHistory));
        } catch {}
    }, []);

    /** 导航输入历史（up=更早, down=更新） */
    const navigateHistory = useCallback((direction: 'up' | 'down') => {
        const history = inputHistoryRef.current;
        if (history.length === 0) return;
        const currentIndex = historyIndexRef.current;
        let newIndex: number;

        if (direction === 'up') {
            if (currentIndex === -1) {
                draftRef.current = extractContent().text;
                newIndex = 0;
            } else {
                newIndex = Math.min(currentIndex + 1, history.length - 1);
            }
        } else {
            if (currentIndex === -1) return;
            newIndex = currentIndex - 1;
            if (newIndex < 0) {
                historyIndexRef.current = -1;
                setEditorText(draftRef.current);
                return;
            }
        }
        historyIndexRef.current = newIndex;
        setEditorText(history[newIndex]);
    }, [extractContent, setEditorText]);

    /** 从历史记录弹窗选择一条记录 */
    const handleHistorySelect = useCallback((text: string) => {
        setEditorText(text);
        const index = inputHistoryRef.current.indexOf(text);
        historyIndexRef.current = index >= 0 ? index : -1;
        setHistoryModalOpen(false);
        setHistorySearchFilter('');
        setTimeout(() => {
            try { ReactEditor.focus(editor); } catch {}
        }, 100);
    }, [editor, setEditorText]);

    /** 重置历史导航索引（用户输入新内容或发送消息后调用） */
    const resetHistoryIndex = useCallback(() => {
        historyIndexRef.current = -1;
    }, []);

    /** 打开历史记录弹窗（Ctrl+R 触发，无记录时静默不打开） */
    const openHistoryModal = useCallback(() => {
        const history = inputHistoryRef.current;
        if (history.length === 0) return;
        setHistoryListForModal([...history]);
        setHistorySearchFilter('');
        setHistoryHoverIndex(-1);
        setHistoryModalOpen(true);
    }, []);

    /** 关闭历史记录弹窗并清空搜索词 */
    const closeHistoryModal = useCallback(() => {
        setHistoryModalOpen(false);
        setHistorySearchFilter('');
    }, []);

    return {
        // refs（供 handleChange / handleSubmit 等读最新值）
        inputHistoryRef,
        historyIndexRef,
        isNavigatingRef,
        // 编辑器工具（handleSubmit 也需要用）
        extractContent,
        setEditorText,
        // Modal UI 状态
        historyModalOpen,
        historySearchFilter,
        historyListForModal,
        historyHoverIndex,
        // actions
        saveToHistory,
        navigateHistory,
        handleHistorySelect,
        resetHistoryIndex,
        openHistoryModal,
        closeHistoryModal,
        setHistorySearchFilter,
        setHistoryHoverIndex,
    };
}
