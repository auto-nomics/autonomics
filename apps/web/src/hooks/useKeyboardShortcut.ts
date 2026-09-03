/**
 * 全局键盘快捷键 Hook
 *
 * 统一管理键盘快捷键的注册和清理，避免在多个组件中重复编写相同的逻辑。
 *
 * 功能特性：
 * - 自动排除 INPUT/TEXTAREA/contentEditable 元素，避免干扰用户输入
 * - 支持自定义按键匹配函数（支持单键、组合键、多键等复杂场景）
 * - 自动清理事件监听器，防止内存泄漏
 * - 支持依赖数组变化时重新注册快捷键
 * - 多面板模式下自动仅响应 active pane 的快捷键
 *   （通过 PaneIdContext 闭包捕获每个组件所属面板 ID）
 *
 * @param {Function} keyMatcher - 按键匹配函数，接收 KeyboardEvent，返回 true 时触发 handler
 * @param {Function} handler - 触发时的回调函数（可选，如果 keyMatcher 已处理逻辑）
 * @param {Array} deps - 依赖数组，与 useEffect 的 deps 相同
 * @param {Object} options - 可选配置
 * @param {boolean} options.excludeInputs - 是否排除输入框（默认 true）
 */
import { useEffect, useContext } from 'react';
import usePaneStore from '../stores/usePaneStore';
import { PaneIdContext } from '../panes/PaneContext';
import { hasMultiplePanes } from '../panes/treeUtils';

export default function useKeyboardShortcut(keyMatcher: (e: KeyboardEvent) => boolean, handler?: (e: KeyboardEvent) => void, deps?: React.DependencyList, options: { excludeInputs?: boolean } = {}) {
  const { excludeInputs = true } = options;

  // 通过 Context 闭包捕获当前组件所属面板 ID
  // 每个面板内的组件拿到不同的 paneId，监听器闭包内据此判断是否响应
  const paneId = useContext(PaneIdContext);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // 排除输入框内的按键，避免干扰用户输入
      if (excludeInputs) {
        const tag = document.activeElement?.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA') return;
        if ((document.activeElement as HTMLElement)?.isContentEditable) return;
      }

      // 多面板模式：仅当前组件所属面板为 active 时才响应
      // paneId 来自 Context 闭包，每个面板的组件有不同的值
      if (paneId !== null) {
        const { activePaneId, root } = usePaneStore.getState();
        if (hasMultiplePanes(root) && paneId !== activePaneId) return;
      }

      // 检查按键是否匹配
      if (keyMatcher(e)) {
        if (handler) {
          handler(e);
        }
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [keyMatcher, handler, ...(deps || []), excludeInputs, paneId]);
}
