/**
 * useReaderShortcuts Hook
 *
 * 管理论文阅读页面的键盘快捷键。
 *
 * 快捷键：
 * - Ctrl+ArrowLeft: 切换左侧导航面板
 * - Ctrl+ArrowRight: 切换右侧聊天面板
 *
 * 在输入框或 contentEditable 元素获得焦点时不会触发快捷键。
 */

import { useCallback } from 'react';
import useKeyboardShortcut from '../../../hooks/useKeyboardShortcut';

interface UseReaderShortcutsProps {
  paperId: string;             // 论文ID
  navigate: (path: string) => void;  // 路由导航函数
  onToggleGuide: () => void;   // 切换导航面板的回调
  onToggleChat: () => void;    // 切换聊天面板的回调
}

/**
 * 论文阅读器快捷键自定义 Hook
 */
export function useReaderShortcuts({
  paperId,
  navigate,
  onToggleGuide,
  onToggleChat,
}: UseReaderShortcutsProps) {
  // 快捷键匹配处理函数
  const handler = useCallback((e: KeyboardEvent) => {
    // 检测 Ctrl+ArrowLeft 组合键
    if (e.ctrlKey && e.key === 'ArrowLeft') {
      return true;
    // 检测 Ctrl+ArrowRight 组合键
    } else if (e.ctrlKey && e.key === 'ArrowRight') {
      return true;
    }
    return false;
  }, []);

  // 快捷键触发回调函数
  const callback = useCallback((e: KeyboardEvent) => {
    e.preventDefault();  // 阻止默认行为
    if (e.key === 'ArrowLeft') {
      onToggleGuide();  // 切换导航面板
    } else if (e.key === 'ArrowRight') {
      onToggleChat();   // 切换聊天面板
    }
  }, [navigate, onToggleGuide, onToggleChat]);

  // 注册快捷键监听
  useKeyboardShortcut(handler, callback, [paperId, navigate, onToggleGuide, onToggleChat]);
}
