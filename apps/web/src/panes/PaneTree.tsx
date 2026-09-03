/**
 * 分栏树递归渲染组件
 *
 * 分栏树的递归渲染器，根据节点类型分发到不同的子组件：
 * - SplitNode（分割节点）→ 渲染 SplitContainer（包含两个子面板和分隔条）
 * - LeafNode（叶子节点）→ 渲染 PaneLeaf（实际内容容器）
 *
 * 这是分栏树渲染的核心递归组件，PaneManager 通过它遍历整棵树并渲染所有节点。
 * 使用 React.memo 优化性能，避免在其他节点状态变化时重渲染。
 *
 * @module panes/PaneTree
 */
import React from 'react';
import { PaneNode, PaneId } from './types';
import PaneLeaf from './PaneLeaf';
import SplitContainer from './SplitContainer';

/**
 * PaneTree 组件的 Props 接口
 */
interface PaneTreeProps {
  /** 当前渲染的节点（可以是分割节点或叶子节点） */
  node: PaneNode;
  /** 是否存在多个 pane（用于决定是否显示活跃/非活跃边框样式） */
  hasMultiplePanes: boolean;
  /** 被最大化的面板 ID（可选，用于最大化模式布局计算） */
  maximizedPaneId?: PaneId | null;
}

/**
 * 分栏树递归渲染组件
 *
 * 根据节点类型递归渲染分栏树的每一层：
 * - 分割节点：渲染 SplitContainer，它会递归渲染两个子节点
 * - 叶子节点：渲染 PaneLeaf，它是实际承载路由内容的终端节点
 *
 * @param {PaneTreeProps} props - 组件属性
 * @returns {React.ReactElement} 渲染的节点组件
 */
function PaneTree({ node, hasMultiplePanes, maximizedPaneId }: PaneTreeProps) {
  // 分割节点：渲染分栏容器（包含两个子面板和可拖拽分隔条）
  if (node.type === 'split') {
    return <SplitContainer splitNode={node} hasMultiplePanes={hasMultiplePanes} maximizedPaneId={maximizedPaneId} />;
  }
  // 叶子节点：渲染分栏叶子（实际内容容器）
  return <PaneLeaf leaf={node} hasMultiplePanes={hasMultiplePanes} />;
}

// 使用 React.memo 优化，避免在其他 pane 状态变化时重渲染
export default React.memo(PaneTree);
