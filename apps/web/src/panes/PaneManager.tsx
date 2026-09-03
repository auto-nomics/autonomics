/**
 * 分栏管理器组件
 *
 * 分栏系统的核心布局引擎，负责：
 * 1. 递归计算分栏树中所有节点的绝对位置和尺寸
 * 2. 渲染叶子节点（PaneLeaf）和可拖拽的分隔条（ResizeHandle）
 * 3. 支持面板最大化功能（单面板全屏显示）
 * 4. 监听容器尺寸变化，动态调整布局
 * 5. 显示前缀键操作提示（PrefixKeyIndicator）
 *
 * 布局算法说明：
 * - 使用绝对定位（position: absolute）实现分栏布局
 * - 递归遍历分栏树，根据 splitRatio 计算每个节点的位置和尺寸
 * - 分隔条（divider）占据 DIVIDER_SIZE 像素的空间
 * - 最大化模式下，包含目标面板的子树占据全部空间，其他面板隐藏
 *
 * @module panes/PaneManager
 */
import React, { useEffect, useRef, useState, useCallback } from 'react';
import usePaneStore from '../stores/usePaneStore';
import { hasMultiplePanes, getAllLeafIds } from './treeUtils';
import PaneLeaf from './PaneLeaf';
import PrefixKeyIndicator from './PrefixKeyIndicator';
import { PaneNode, PaneId, LeafNode } from './types';

/**
 * 分隔条尺寸（像素）
 *
 * 分隔条占据的固定宽度/高度，用于在两个相邻面板之间提供可拖拽的调整区域。
 * 实际可拖拽区域通过 ResizeHandle 组件实现。
 */
const DIVIDER_SIZE = 4;

/**
 * 叶子节点的布局信息接口
 *
 * 描述一个叶子节点在容器中的位置和尺寸。
 */
interface LeafLayout {
  /** 叶子节点数据 */
  leaf: LeafNode;
  /** 节点的矩形区域：x=左上角X坐标, y=左上角Y坐标, w=宽度, h=高度 */
  rect: { x: number; y: number; w: number; h: number };
}

/**
 * 分隔条的布局信息接口
 *
 * 描述一个分隔条在容器中的位置、方向和关联的分割比例。
 */
interface DividerLayout {
  /** 关联的分割节点 ID */
  splitId: PaneId;
  /** 分割方向：horizontal=水平分割（左右排列）, vertical=垂直分割（上下排列） */
  direction: 'horizontal' | 'vertical';
  /** 分隔条的矩形区域 */
  rect: { x: number; y: number; w: number; h: number };
  /** 关联的分割比例（0-1），用于拖拽调整时的初始值 */
  splitRatio: number;
}

/**
 * 递归计算分栏树中所有节点的布局
 *
 * 遍历分栏树，为每个叶子节点和分隔条计算绝对位置和尺寸。
 * 计算规则：
 * - 叶子节点：直接使用传入的矩形区域
 * - 分割节点：根据 splitRatio 将空间分为两部分，递归计算子节点
 * - 最大化模式：包含目标面板的子树占据全部空间，其他面板尺寸为 0
 *
 * @param {PaneNode} node - 当前遍历的节点
 * @param {number} x - 节点左上角 X 坐标
 * @param {number} y - 节点左上角 Y 坐标
 * @param {number} w - 节点可用宽度
 * @param {number} h - 节点可用高度
 * @param {PaneId | null} [maximizedPaneId] - 被最大化的面板 ID（可选）
 * @returns {{ leaves: LeafLayout[]; dividers: DividerLayout[] }} 所有叶子节点和分隔条的布局信息
 */
function computeLayouts(
  node: PaneNode,
  x: number, y: number, w: number, h: number,
  maximizedPaneId?: PaneId | null
): { leaves: LeafLayout[]; dividers: DividerLayout[] } {
  // 基础情况：叶子节点，直接返回其布局信息
  if (node.type === 'leaf') {
    return {
      leaves: [{ leaf: node, rect: { x, y, w, h } }],
      dividers: [],
    };
  }

  // 最大化模式处理：
  // 如果存在被最大化的面板，递归查找其所在的子树，
  // 包含目标面板的子树占据全部空间，其他子树尺寸为 0
  if (maximizedPaneId) {
    // 获取左子树和右子树的所有叶子节点 ID
    const firstIds = getAllLeafIds(node.first);
    const secondIds = getAllLeafIds(node.second);
    // 如果左子树包含被最大化的面板
    if (firstIds.includes(maximizedPaneId)) {
      // 左子树占据全部空间，右子树隐藏（尺寸为 0）
      const visible = computeLayouts(node.first, x, y, w, h, maximizedPaneId);
      const hidden = computeLayouts(node.second, 0, 0, 0, 0, null);
      return { leaves: [...visible.leaves, ...hidden.leaves], dividers: visible.dividers };
    }
    // 如果右子树包含被最大化的面板
    if (secondIds.includes(maximizedPaneId)) {
      // 左子树隐藏（尺寸为 0），右子树占据全部空间
      const hidden = computeLayouts(node.first, 0, 0, 0, 0, null);
      const visible = computeLayouts(node.second, x, y, w, h, maximizedPaneId);
      return { leaves: [...hidden.leaves, ...visible.leaves], dividers: visible.dividers };
    }
  }

  // 常规分割模式：
  // 根据 splitRatio 将当前空间分为两部分，中间放置分隔条
  const isHorizontal = node.direction === 'horizontal';
  const halfD = DIVIDER_SIZE / 2; // 分隔条半宽/半高

  // 计算左右（或上下）两个子区域的尺寸
  // 分隔条的空间从两个子区域中各扣除一半
  const firstSize = isHorizontal
    ? w * node.splitRatio - halfD // 水平分割：左子区域宽度
    : h * node.splitRatio - halfD; // 垂直分割：上子区域高度
  const secondSize = isHorizontal
    ? w * (1 - node.splitRatio) - halfD // 水平分割：右子区域宽度
    : h * (1 - node.splitRatio) - halfD; // 垂直分割：下子区域高度

  // 计算左（上）子区域的矩形
  const firstRect = isHorizontal
    ? { x, y, w: firstSize, h }
    : { x, y, w, h: firstSize };
  // 计算分隔条的矩形
  const dividerRect = isHorizontal
    ? { x: x + firstSize, y, w: DIVIDER_SIZE, h }
    : { x, y: y + firstSize, w, h: DIVIDER_SIZE };
  // 计算右（下）子区域的矩形
  const secondRect = isHorizontal
    ? { x: x + firstSize + DIVIDER_SIZE, y, w: secondSize, h }
    : { x, y: y + firstSize + DIVIDER_SIZE, w, h: secondSize };

  // 递归计算两个子区域的布局
  const first = computeLayouts(node.first, firstRect.x, firstRect.y, firstRect.w, firstRect.h);
  const second = computeLayouts(node.second, secondRect.x, secondRect.y, secondRect.w, secondRect.h);

  // 合并结果：叶子节点数组 + 左子树分隔条 + 当前分隔条 + 右子树分隔条
  return {
    leaves: [...first.leaves, ...second.leaves],
    dividers: [
      ...first.dividers,
      { splitId: node.id, direction: node.direction, rect: dividerRect, splitRatio: node.splitRatio },
      ...second.dividers,
    ],
  };
}

/**
 * 分隔条拖拽调整组件
 *
 * 渲染一个可拖拽的分隔条，用于调整相邻两个面板的分割比例。
 * 拖拽过程中实时更新 store 中的 splitRatio，实现面板尺寸的动态调整。
 *
 * 拖拽实现原理：
 * 1. 鼠标按下时记录起始位置和初始分割比例
 * 2. 鼠标移动时计算偏移量，转换为比例变化，更新 store
 * 3. 鼠标松开时清理事件监听器，恢复鼠标样式
 *
 * @param {DividerLayout & { containerWidth: number; containerHeight: number }} props
 * @param {PaneId} props.splitId - 关联的分割节点 ID
 * @param {string} props.direction - 分割方向
 * @param {object} props.rect - 分隔条的矩形区域
 * @param {number} props.splitRatio - 初始分割比例
 * @param {number} props.containerWidth - 容器宽度（用于计算比例）
 * @param {number} props.containerHeight - 容器高度（用于计算比例）
 */
function ResizeHandle({ splitId, direction, rect, splitRatio, containerWidth, containerHeight }: DividerLayout & { containerWidth: number; containerHeight: number }) {
  // 判断是否为水平分割（左右排列）
  const isHorizontal = direction === 'horizontal';
  // 拖拽状态标记
  const [isDragging, setIsDragging] = useState(false);

  /**
   * 处理鼠标按下事件，开始拖拽
   *
   * 拖拽过程中：
   * - 监听 mousemove 事件，实时计算新的分割比例
   * - 监听 mouseup 事件，结束拖拽并清理
   * - 修改全局鼠标样式和禁止文本选择
   */
  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault(); // 阻止默认行为（如文本选择）
    setIsDragging(true); // 设置拖拽状态
    const startPos = isHorizontal ? e.clientX : e.clientY; // 记录起始鼠标位置
    const containerSize = isHorizontal ? containerWidth : containerHeight; // 获取容器尺寸
    const startRatio = splitRatio; // 记录初始分割比例

    /**
     * 鼠标移动事件处理：计算新的分割比例
     * 公式：新比例 = 初始比例 + 鼠标偏移量 / 容器尺寸
     */
    const handleMouseMove = (moveE: MouseEvent) => {
      const currentPos = isHorizontal ? moveE.clientX : moveE.clientY;
      const newRatio = startRatio + (currentPos - startPos) / containerSize;
      // 更新 store 中的分割比例（带边界限制）
      usePaneStore.getState().setSplitRatio(splitId, newRatio);
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
  }, [splitId, splitRatio, isHorizontal, containerWidth, containerHeight]);

  return (
    <div
      onMouseDown={handleMouseDown} // 鼠标按下开始拖拽
      className={`pane-resize-divider ${isHorizontal ? 'pane-resize-divider-horizontal' : 'pane-resize-divider-vertical'}`}
      style={{
        position: 'absolute', // 绝对定位
        left: rect.x, // X 坐标
        top: rect.y, // Y 坐标
        width: rect.w, // 宽度
        height: rect.h, // 高度
        // 拖拽时高亮显示分隔条（主题色）
        background: isDragging ? 'var(--color-primary)' : undefined,
      }}
    />
  );
}

/**
 * 分栏管理器组件（默认导出）
 *
 * 分栏系统的顶层布局容器，负责：
 * 1. 监听容器尺寸变化（ResizeObserver）
 * 2. 调用 computeLayouts 计算所有节点的布局
 * 3. 渲染叶子节点和分隔条
 * 4. 显示前缀键操作提示
 *
 * @returns {React.ReactElement} 分栏管理器的 JSX
 */
export default function PaneManager() {
  // 从 store 获取分栏树根节点
  const root = usePaneStore(s => s.root);
  // 从 store 获取前缀键状态（idle/active/pendingSplit）
  const prefixKeyState = usePaneStore(s => s.prefixKeyState);
  // 从 store 获取被最大化的面板 ID
  const maximizedPaneId = usePaneStore(s => s.maximizedPaneId);
  // 判断是否存在多个 pane
  const multi = hasMultiplePanes(root);

  // 容器 DOM 引用，用于监听尺寸变化
  const containerRef = useRef<HTMLDivElement>(null);
  // 容器尺寸状态
  const [size, setSize] = useState({ width: 0, height: 0 });

  /**
   * 监听容器尺寸变化
   * 使用 ResizeObserver API 监测容器尺寸变化，
   * 自动更新 size 状态，触发重新计算布局
   */
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    // 创建 ResizeObserver 实例
    const ro = new ResizeObserver(entries => {
      const { width, height } = entries[0].contentRect;
      setSize({ width, height }); // 更新容器尺寸
    });
    ro.observe(el); // 开始观察容器元素
    return () => ro.disconnect(); // 清理：停止观察
  }, []);

  // 计算所有节点的布局
  // 多 pane 模式下支持最大化，单 pane 模式下忽略最大化
  const { leaves, dividers } = computeLayouts(
    root, 0, 0, size.width, size.height,
    multi ? maximizedPaneId : null
  );

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column', position: 'relative' }}>
      {/* 布局容器：包含所有叶子节点和分隔条 */}
      <div ref={containerRef} style={{ flex: 1, overflow: 'hidden', position: 'relative' }}>
        {/* 渲染所有叶子节点 */}
        {size.width > 0 && leaves.map(({ leaf, rect }) => {
          // 判断节点是否被隐藏（最大化模式下尺寸为 0 的节点）
          const isHidden = maximizedPaneId != null && rect.w === 0 && rect.h === 0;
          return (
            <div key={leaf.id} style={{
              // 隐藏的节点不使用绝对定位（从文档流中移除）
              position: isHidden ? undefined : 'absolute',
              left: isHidden ? undefined : rect.x,
              top: isHidden ? undefined : rect.y,
              width: isHidden ? undefined : rect.w,
              height: isHidden ? undefined : rect.h,
              overflow: 'hidden',
              display: isHidden ? 'none' : undefined, // 完全隐藏
            }}>
              <PaneLeaf leaf={leaf} hasMultiplePanes={multi} />
            </div>
          );
        })}
        {/* 渲染可拖拽的分隔条（仅在多 pane 且非最大化模式下显示） */}
        {multi && !maximizedPaneId && size.width > 0 && dividers.map(d => (
          <ResizeHandle key={d.splitId} {...d} containerWidth={size.width} containerHeight={size.height} />
        ))}
      </div>
      {/* 前缀键操作提示（仅在非 idle 状态下显示） */}
      {prefixKeyState !== 'idle' && <PrefixKeyIndicator state={prefixKeyState} />}
    </div>
  );
}
