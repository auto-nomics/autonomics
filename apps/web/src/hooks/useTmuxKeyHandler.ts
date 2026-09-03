/**
 * tmux 风格键盘快捷键处理 Hook
 *
 * 实现 tmux 风格的键盘快捷键系统：
 * - Ctrl+B 激活前缀键模式
 * - 前缀键后可执行分屏（h/j/k/l）、导航（方向键）、跳转路由（a/p）等操作
 * - Alt+F 切换最大化面板
 * - Alt+W 关闭当前分屏
 *
 * 快捷键说明：
 * - Ctrl+B: 激活前缀键模式
 * - h/l/k/j: 分屏（左/右/上/下）
 * - 方向键: 在相邻面板间导航
 * - x: 关闭当前面板
 * - d: 切换深色/浅色主题
 * - a/p: 导航到对应路由
 *
 * @module useTmuxKeyHandler
 */
import { useEffect } from 'react';
import usePaneStore from '../stores/usePaneStore';
import useAppStore from '../stores/useAppStore';
import { getAllLeafIds, findAdjacentLeaf, leafCount } from '../panes/treeUtils';

// 导航键到路由的映射
const ROUTE_MAP: Record<string, string> = {
  'a': '/',
  'p': '/papers',
};

export default function useTmuxKeyHandler() {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const state = usePaneStore.getState();
      const { prefixKeyState, activatePrefixKey, cancelPrefixKey } = state;

      // Alt+F: 切换最大化面板（始终生效，与任何编辑器无冲突）
      if (e.altKey && !e.ctrlKey && !e.shiftKey && e.key === 'f') {
        e.preventDefault();
        usePaneStore.getState().toggleMaximize();
        return;
      }

      // Alt+W: 关闭当前分屏
      if (e.altKey && !e.ctrlKey && !e.shiftKey && e.key === 'w') {
        e.preventDefault();
        const { root, activePaneId, closePane } = usePaneStore.getState();
        if (leafCount(root) > 1) closePane(activePaneId);
        return;
      }

      // Tiptap 富文本编辑器中：Ctrl+B 保留给加粗，不拦截前缀键
      // Slate 输入框虽为 contentEditable 但无加粗功能，Ctrl+B 仍应激活前缀键
      const el = document.activeElement;
      if (el?.closest?.('.tiptap')) return;

      // 处理 pendingSplit 状态
      if (prefixKeyState === 'pendingSplit') {
        e.preventDefault();
        const store = usePaneStore.getState();
        const route = ROUTE_MAP[e.key];
        if (route) {
          store.executePendingSplit(route);
        } else if (e.key === 'Escape') {
          store.clearPendingSplit();
        } else {
          store.executePendingSplit();
        }
        return;
      }

      // Ctrl+B: 在纯文本输入框（INPUT/TEXTAREA）中激活 prefix（无加粗功能）
      if (prefixKeyState === 'idle') {
        if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key === 'b') {
          e.preventDefault();
          activatePrefixKey();
        }
        return;
      }

      // 前缀键已激活：命令键始终响应（preventDefault 阻止文字输入）
      e.preventDefault();

      const { root, activePaneId, splitPane, closePane, setActivePane } = usePaneStore.getState();
      const count = leafCount(root);

      switch (e.key) {
        case 'h': // 向左分屏 → 进入 pendingSplit 等待可能的导航键
          if (count < 8) usePaneStore.getState().setPendingSplit('horizontal', 'before');
          else cancelPrefixKey();
          break;
        case 'l': // 向右分屏 → 进入 pendingSplit 等待可能的导航键
          if (count < 8) usePaneStore.getState().setPendingSplit('horizontal', 'after');
          else cancelPrefixKey();
          break;
        case 'k': // 向上分屏 → 进入 pendingSplit 等待可能的导航键
          if (count < 8) usePaneStore.getState().setPendingSplit('vertical', 'before');
          else cancelPrefixKey();
          break;
        case 'j': // 向下分屏 → 进入 pendingSplit 等待可能的导航键
          if (count < 8) usePaneStore.getState().setPendingSplit('vertical', 'after');
          else cancelPrefixKey();
          break;
        case 'ArrowLeft': { // 激活左侧相邻面板
          cancelPrefixKey();
          const target = findAdjacentLeaf(root, activePaneId, 'left');
          if (target) setActivePane(target);
          break;
        }
        case 'ArrowRight': { // 激活右侧相邻面板
          cancelPrefixKey();
          const target = findAdjacentLeaf(root, activePaneId, 'right');
          if (target) setActivePane(target);
          break;
        }
        case 'ArrowUp': { // 激活上方相邻面板
          cancelPrefixKey();
          const target = findAdjacentLeaf(root, activePaneId, 'up');
          if (target) setActivePane(target);
          break;
        }
        case 'ArrowDown': { // 激活下方相邻面板
          cancelPrefixKey();
          const target = findAdjacentLeaf(root, activePaneId, 'down');
          if (target) setActivePane(target);
          break;
        }
        case 'x': // 关闭当前面板
          cancelPrefixKey();
          if (count > 1) closePane(activePaneId);
          break;
        case 'd': // 切换深色/浅色主题
          cancelPrefixKey();
          useAppStore.getState().toggleTheme();
          break;
        case 'a': // 跳转到首页
          cancelPrefixKey();
          usePaneStore.getState().navigateActivePane('/');
          break;
        case 'p': // 跳转到论文列表
          cancelPrefixKey();
          usePaneStore.getState().navigateActivePane('/papers');
          break;
        default: // 未知按键，取消前缀键模式
          cancelPrefixKey();
          break;
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, []);
}
