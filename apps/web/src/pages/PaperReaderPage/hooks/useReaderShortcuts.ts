/**
 * useReaderShortcuts Hook
 *
 * 管理论文阅读页面的键盘快捷键。
 *
 * 快捷键：
 * - Ctrl+ArrowRight: 切换右侧聊天面板
 *
 * 在输入框或 contentEditable 元素获得焦点时不会触发快捷键。
 *
 * 注：Ctrl+ArrowLeft 原本切换左侧导览面板；导览面板已随 guideApi 移除，
 * 该快捷键一并删除（plan Phase 5 清扫）。
 */

import { useCallback } from 'react';
import useKeyboardShortcut from '../../../hooks/useKeyboardShortcut';

interface UseReaderShortcutsProps {
  paperId: string;             // 论文ID
  navigate: (path: string) => void;  // 路由导航函数
  onToggleChat: () => void;    // 切换聊天面板的回调
}

/**
 * 论文阅读器快捷键自定义 Hook
 */
export function useReaderShortcuts({
  paperId,
  navigate,
  onToggleChat,
}: UseReaderShortcutsProps) {
  // 快捷键匹配处理函数
  const handler = useCallback((e: KeyboardEvent) => {
    // 检测 Ctrl+ArrowRight 组合键
    return e.ctrlKey && e.key === 'ArrowRight';
  }, []);

  // 快捷键触发回调函数
  const callback = useCallback((e: KeyboardEvent) => {
    e.preventDefault();  // 阻止默认行为
    onToggleChat();      // 切换聊天面板
  }, [onToggleChat]);

  // 注册快捷键监听
  useKeyboardShortcut(handler, callback, [paperId, navigate, onToggleChat]);
}
