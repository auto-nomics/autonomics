/**
 * 分栏上下文管理模块
 *
 * 提供分栏系统的核心上下文机制：
 * - PaneIdContext: React Context，用于在分栏树中标识当前 pane 的 ID
 * - useIsActivePane: 自定义 Hook，检测当前组件是否在活跃 pane 中
 *
 * 设计说明：
 * - 使用 React Context 而非 props drilling，避免逐层传递 paneId
 * - useIsActivePane 在单 pane 模式下始终返回 true，简化调用方逻辑
 * - 活跃 pane 的判断依赖 usePaneStore 中的 activePaneId 状态
 *
 * @module panes/PaneContext
 */
import { createContext, useContext } from 'react';
import usePaneStore from '../stores/usePaneStore';
import { hasMultiplePanes } from '../panes/treeUtils';

/**
 * Pane ID 上下文
 *
 * 在 PaneLeaf 组件中通过 Provider 注入当前 pane 的 ID，
 * 子组件可通过 useContext(PaneIdContext) 获取所在 pane 的 ID。
 *
 * 默认值为 null，表示不在任何 pane 中（如顶层组件）。
 */
export const PaneIdContext = createContext<string | null>(null);

/**
 * 检测当前组件是否在活跃 pane 中的 Hook
 *
 * 用于判断当前组件是否应该响应键盘事件和执行操作。
 * 单 pane 模式下始终返回 true（无需区分活跃状态）。
 * 多 pane 模式下，只有当前 pane ID 与 store 中的 activePaneId 匹配时返回 true。
 *
 * @returns {boolean} 当前组件是否在活跃 pane 中
 *
 * @example
 * function MyComponent() {
 *   const isActive = useIsActivePane();
 *   // 只在活跃 pane 中响应键盘事件
 *   useEffect(() => {
 *     if (!isActive) return;
 *     // 注册键盘快捷键...
 *   }, [isActive]);
 * }
 */
export function useIsActivePane(): boolean {
  // 从 Context 获取当前 pane ID
  const paneId = useContext(PaneIdContext);
  // 从 store 获取当前活跃 pane ID
  const activePaneId = usePaneStore(s => s.activePaneId);
  // 从 store 获取分栏树根节点
  const root = usePaneStore(s => s.root);

  // 单 pane 模式下始终返回 true，无需区分活跃状态
  if (!hasMultiplePanes(root)) return true;
  // 多 pane 模式下，比较当前 pane ID 与活跃 pane ID
  return paneId === activePaneId;
}
