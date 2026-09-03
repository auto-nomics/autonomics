/**
 * 分栏容器组件
 *
 * 渲染分栏树中的分割节点（SplitNode），包含两个子面板和可拖拽的分隔条。
 * 是分栏系统中实现面板分割和尺寸调整的核心组件。
 *
 * 功能特性：
 * - 支持水平分割（左右排列）和垂直分割（上下排列）
 * - 可拖拽的分隔条，实时调整两个面板的分割比例
 * - 面板最大化功能：双击分隔条可最大化某一侧面板
 * - 拖拽时高亮显示分隔条（主题色反馈）
 *
 * 布局实现：
 * - 使用 flex 布局，通过 flex 属性控制两个子面板的比例
 * - 分隔条通过 flexShrink: 0 保持固定尺寸
 * - 最大化模式下，目标侧 flex=1，另一侧 flex=0 且 display=none
 *
 * @module panes/SplitContainer
 */
import React, { useCallback, useRef, useState } from 'react';
import { SplitNode, PaneId } from './types';
import { getAllLeafIds } from './treeUtils';
import usePaneStore from '../stores/usePaneStore';
import PaneTree from './PaneTree';

/**
 * SplitContainer 组件的 Props 接口
 */
interface SplitContainerProps {
  /** 分割节点数据，包含方向、比例和两个子节点 */
  splitNode: SplitNode;
  /** 是否存在多个 pane（用于决定是否显示活跃/非活跃边框样式） */
  hasMultiplePanes: boolean;
  /** 被最大化的面板 ID（可选，用于最大化模式布局） */
  maximizedPaneId?: PaneId | null;
}

/**
 * 分栏容器组件（默认导出）
 *
 * 渲染一个分割节点，包含左右（或上下）两个子面板和可拖拽的分隔条。
 *
 * @param {SplitContainerProps} props - 组件属性
 * @returns {React.ReactElement} 分栏容器的 JSX
 */
export default function SplitContainer({ splitNode, hasMultiplePanes, maximizedPaneId }: SplitContainerProps) {
  // 容器 DOM 引用，用于获取容器尺寸
  const containerRef = useRef<HTMLDivElement>(null);
  // 拖拽状态标记
  const [isDragging, setIsDragging] = useState(false);

  // 判断分割方向：水平（左右）或垂直（上下）
  const isHorizontal = splitNode.direction === 'horizontal';
  // 获取左（上）子树的所有叶子节点 ID
  const firstLeafIds = getAllLeafIds(splitNode.first);
  // 获取右（下）子树的所有叶子节点 ID
  const secondLeafIds = getAllLeafIds(splitNode.second);

  /**
   * 最大化状态判断逻辑
   *
   * - firstContainsMax: 左（上）子树是否包含被最大化的面板
   * - secondContainsMax: 右（下）子树是否包含被最大化的面板
   * - isMaximized: 当前分割容器是否有任何子节点被最大化
   *
   * 实现逻辑：当一侧包含最大化节点时，该侧 flex=1 展开，另一侧 flex=0 完全折叠
   */
  const firstContainsMax = !!maximizedPaneId && firstLeafIds.includes(maximizedPaneId);
  const secondContainsMax = !!maximizedPaneId && secondLeafIds.includes(maximizedPaneId);
  const isMaximized = !!(firstContainsMax || secondContainsMax);

  /**
   * 处理分隔条拖拽开始事件
   *
   * 拖拽流程：
   * 1. 记录鼠标起始位置和初始分割比例
   * 2. 监听 mousemove 事件，实时计算新的分割比例并更新 store
   * 3. 监听 mouseup 事件，结束拖拽并清理事件监听器
   * 4. 修改全局鼠标样式和禁止文本选择
   */
  const handleResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault(); // 阻止默认行为
    const container = containerRef.current;
    if (!container) return;

    // 记录起始鼠标位置
    const startPos = isHorizontal ? e.clientX : e.clientY;
    // 获取容器尺寸（用于计算比例变化）
    const containerSize = isHorizontal ? container.offsetWidth : container.offsetHeight;
    // 记录初始分割比例
    const startRatio = splitNode.splitRatio;

    setIsDragging(true); // 设置拖拽状态

    /**
     * 鼠标移动事件处理：计算新的分割比例
     * 公式：新比例 = 初始比例 + 鼠标偏移量 / 容器尺寸
     */
    const handleMouseMove = (moveE: MouseEvent) => {
      const currentPos = isHorizontal ? moveE.clientX : moveE.clientY;
      const delta = currentPos - startPos; // 鼠标偏移量
      const deltaRatio = delta / containerSize; // 偏移比例
      const newRatio = startRatio + deltaRatio; // 新的分割比例
      usePaneStore.getState().setSplitRatio(splitNode.id, newRatio); // 更新 store
    };

    /**
     * 鼠标松开事件处理：结束拖拽
     * 清理事件监听器，恢复鼠标样式和文本选择
     */
    const handleMouseUp = () => {
      setIsDragging(false); // 清除拖拽状态
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      document.body.style.cursor = ''; // 恢复默认鼠标样式
      document.body.style.userSelect = ''; // 恢复文本选择
    };

    // 设置全局鼠标样式（拖拽方向指示）
    document.body.style.cursor = isHorizontal ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none'; // 禁止文本选择
    // 注册全局事件监听器
    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
  }, [splitNode.id, splitNode.splitRatio, isHorizontal]);

  return (
    <div
      ref={containerRef} // 容器 DOM 引用
      style={{
        display: 'flex', // flex 布局
        flexDirection: isHorizontal ? 'row' : 'column', // 分割方向
        height: '100%', // 占满父容器高度
        width: '100%', // 占满父容器宽度
        overflow: 'hidden', // 隐藏溢出内容
      }}
    >
      {/* 左（上）子面板 */}
      <div style={{
        flex: isMaximized ? (firstContainsMax ? 1 : 0) : splitNode.splitRatio, // 最大化时展开/折叠，否则使用分割比例
        overflow: 'hidden', // 隐藏溢出
        minWidth: 0, // 允许 flex 收缩
        minHeight: 0, // 允许 flex 收缩
        display: isMaximized && !firstContainsMax ? 'none' : undefined, // 最大化时隐藏非目标侧
      }}>
        <PaneTree node={splitNode.first} hasMultiplePanes={hasMultiplePanes} maximizedPaneId={maximizedPaneId} />
      </div>

      {/* 可拖拽的分隔条（最大化模式下隐藏） */}
      {!isMaximized && (
        <div
          onMouseDown={handleResizeStart} // 鼠标按下开始拖拽
          className={`pane-resize-divider ${isHorizontal ? 'pane-resize-divider-horizontal' : 'pane-resize-divider-vertical'}`}
          style={{
            flexShrink: 0, // 保持固定尺寸，不被 flex 压缩
            // 拖拽时高亮显示分隔条（主题色反馈）
            background: isDragging ? 'var(--color-primary)' : undefined,
          }}
        />
      )}

      {/* 右（下）子面板 */}
      <div style={{
        flex: isMaximized ? (secondContainsMax ? 1 : 0) : 1 - splitNode.splitRatio, // 最大化时展开/折叠，否则使用剩余比例
        overflow: 'hidden', // 隐藏溢出
        minWidth: 0, // 允许 flex 收缩
        minHeight: 0, // 允许 flex 收缩
        display: isMaximized && !secondContainsMax ? 'none' : undefined, // 最大化时隐藏非目标侧
      }}>
        <PaneTree node={splitNode.second} hasMultiplePanes={hasMultiplePanes} maximizedPaneId={maximizedPaneId} />
      </div>
    </div>
  );
}
