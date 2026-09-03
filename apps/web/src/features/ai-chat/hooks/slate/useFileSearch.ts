/**
 * 文件搜索 Hook（@触发）
 *
 * 管理 Slate 编辑器中输入 @ 触发的文件搜索弹窗的全部状态：
 * - 可见性 / 结果列表 / 当前选中索引 / loading
 * - 触发位置（targetRange）/ 触发搜索词（query，用于结果回填时删除搜索文本）
 * - 200ms 防抖 + AbortController 中断上一次请求
 *
 * 设计要点（替代原 3 对 twin state/ref）：
 * - 用 useReducer 单一 state source，dispatch 天然稳定（不需要固定 dep 的 useCallback）
 * - 单个 stateRef mirror 让固定 dep 的 handleKeyDown 能同步读最新状态
 *   （原来的 fileSearchVisibleRef / fileSearchResultsRef / fileSearchIndexRef 三个 ref → 一个 stateRef）
 * - searchFiles 异步逻辑收敛在 scheduleSearch，调用方只管 dispatch 触发
 *
 * 抽出原因：原 SlateInputWithSender 内 @ 搜索状态分散在 useState×3 + useRef×3 + 自定义 _set 包装器，
 * 独立成 hook 让 SlateInputWithSender 专注于编辑器/发送主流程。
 *
 * @module ai-chat/hooks/slate/useFileSearch
 */

import { useEffect, useReducer, useRef } from 'react';
import type { Range } from 'slate';
import { searchFiles, type FileSearchResult } from '../../../../services/fileSearchApi';

/** 文件搜索完整状态 */
export interface FileSearchState {
    visible: boolean;
    results: FileSearchResult[];
    index: number;
    loading: boolean;
    /** 触发搜索的文本（如 "readme"），insertFileMention 用它计算要删除的字符数 */
    query: string;
    /** @ 触发位置的 Range，insertFileMention 用它定位要替换的搜索文本 */
    targetRange: Range | null;
}

/** 文件搜索 action */
export type FileSearchAction =
    | { type: 'OPEN_WITH_QUERY'; query: string; targetRange: Range | null }
    | { type: 'OPEN_EMPTY' }
    | { type: 'CLOSE' }
    | { type: 'CLOSE_AND_CLEAR' }
    | { type: 'SET_RESULTS'; results: FileSearchResult[] }
    | { type: 'SET_LOADING'; loading: boolean }
    | { type: 'SET_INDEX'; index: number }
    | { type: 'MOVE_INDEX'; direction: 'up' | 'down' };

const initialState: FileSearchState = {
    visible: false,
    results: [],
    index: 0,
    loading: false,
    query: '',
    targetRange: null,
};

function fileSearchReducer(state: FileSearchState, action: FileSearchAction): FileSearchState {
    switch (action.type) {
        case 'OPEN_WITH_QUERY':
            return { ...state, visible: true, query: action.query, targetRange: action.targetRange, index: 0 };
        case 'OPEN_EMPTY':
            return { ...state, visible: true, results: [], loading: false, index: 0 };
        case 'CLOSE':
            return { ...state, visible: false };
        case 'CLOSE_AND_CLEAR':
            return { ...initialState };
        case 'SET_RESULTS':
            return { ...state, results: action.results, index: 0 };
        case 'SET_LOADING':
            return { ...state, loading: action.loading };
        case 'SET_INDEX':
            return { ...state, index: action.index };
        case 'MOVE_INDEX': {
            const len = state.results.length;
            if (len === 0) return state;
            const nextIdx = action.direction === 'down'
                ? (state.index < len - 1 ? state.index + 1 : 0)
                : (state.index > 0 ? state.index - 1 : len - 1);
            return { ...state, index: nextIdx };
        }
        default:
            return state;
    }
}

/** 防抖延迟（毫秒） */
const SEARCH_DEBOUNCE_MS = 200;

/**
 * 文件搜索 Hook
 *
 * @returns state 当前状态（驱动 UI 渲染），dispatch action 分发器（稳定），stateRef 同步状态镜像（供固定 dep 回调读最新值），scheduleSearch 触发 200ms 防抖搜索
 */
export function useFileSearch(): {
    state: FileSearchState;
    dispatch: React.Dispatch<FileSearchAction>;
    stateRef: React.MutableRefObject<FileSearchState>;
    scheduleSearch: (query: string) => void;
} {
    const [state, dispatch] = useReducer(fileSearchReducer, initialState);

    // 单一 stateRef mirror：替代原 3 对 twin state/ref（visible/Ref、results/Ref、index/Ref）
    // 让固定 dep 的 handleKeyDown 能同步读最新 visible/results/index
    const stateRef = useRef(state);
    useEffect(() => {
        stateRef.current = state;
    }, [state]);

    // 搜索调度：200ms 防抖 + AbortController 中断上一次请求
    const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const abortRef = useRef<AbortController | null>(null);

    const scheduleSearch = (query: string) => {
        if (debounceRef.current) clearTimeout(debounceRef.current);
        dispatch({ type: 'SET_LOADING', loading: true });
        debounceRef.current = setTimeout(async () => {
            try {
                if (abortRef.current) abortRef.current.abort();
                const controller = new AbortController();
                abortRef.current = controller;
                const res = await searchFiles(query);
                if (controller.signal.aborted) return;
                dispatch({ type: 'SET_RESULTS', results: res.results || [] });
            } catch {
                dispatch({ type: 'SET_RESULTS', results: [] });
            } finally {
                dispatch({ type: 'SET_LOADING', loading: false });
            }
        }, SEARCH_DEBOUNCE_MS);
    };

    // 卸载时清理：避免遗留 timer / 中断未完成的请求
    useEffect(() => {
        return () => {
            if (debounceRef.current) clearTimeout(debounceRef.current);
            if (abortRef.current) abortRef.current.abort();
        };
    }, []);

    return { state, dispatch, stateRef, scheduleSearch };
}
