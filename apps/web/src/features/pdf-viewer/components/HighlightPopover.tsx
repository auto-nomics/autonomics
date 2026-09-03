/**
 * HighlightPopover — 高亮标注浮动操作气泡
 *
 * 当用户点击 CSS Custom Highlight 渲染的高亮文本时，
 * 显示此浮动气泡提供删除操作。
 *
 * 设计要点：
 * - 使用 position: fixed 定位在点击位置附近
 * - 不依赖任何 DOM 锚定元素（因为 CSS Custom Highlight 没有可锚定的 DOM 节点）
 * - 使用 React Portal 渲染到 document.body，避免被父容器的 overflow 裁切
 * - 支持点击外部关闭和 Escape 键关闭
 * - 删除操作需要二次确认（使用 antd Popconfirm）
 *
 * 交互流程：
 * 1. 用户点击高亮文本 → 父组件计算点击位置并显示气泡
 * 2. 气泡显示高亮文本摘要 + 删除按钮
 * 3. 用户点击"删除" → 弹出确认对话框
 * 4. 确认删除 → 调用 onDelete 回调 → 气泡关闭
 * 5. 点击气泡外部 / 按 Escape → 气泡关闭
 *
 * @module HighlightPopover
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';  // Portal：将子节点渲染到 DOM 树的其他位置
import { Popconfirm } from 'antd';          // Ant Design 二次确认弹出框
import { DeleteOutlined } from '@ant-design/icons'; // 删除图标

/**
 * 高亮标注浮动操作气泡组件
 *
 * @param {Object} props - 组件属性
 * @param {boolean} props.visible - 气泡是否可见
 * @param {number} props.x - 气泡定位的水平坐标（视口坐标，像素）
 * @param {number} props.y - 气泡定位的垂直坐标（视口坐标，像素）
 * @param {Object|null} props.highlight - 高亮标注数据对象
 *   包含 text（高亮文本）、id（标注 ID）等字段
 * @param {Function} props.onDelete - 删除高亮的回调函数 (highlight: Object) => void
 * @param {Function} props.onClose - 关闭气泡的回调函数 () => void
 * @returns {React.ReactPortal | null} 渲染到 body 的 Portal 或 null
 */
export default function HighlightPopover({
  visible,       // 气泡是否可见
  x,             // 水平坐标
  y,             // 垂直坐标
  highlight,     // 高亮数据对象
  onDelete,      // 删除回调
  onClose,       // 关闭回调
}: {
  visible: boolean;
  x: number;
  y: number;
  highlight: any;
  onDelete: (highlight: any) => void;
  onClose: () => void;
}) {
  // 气泡容器的 DOM 引用，用于点击外部检测
  const popoverRef = useRef<HTMLDivElement>(null);
  // 气泡的实际定位位置（可能会因为视口约束而调整）
  const [popoverPosition, setPopoverPosition] = useState({ x, y });

  /**
   * 删除处理函数（使用 useCallback 缓存，避免不必要的重渲染）
   *
   * 步骤：
   * 1. 检查 highlight 和 onDelete 是否有效
   * 2. 调用 onDelete 将高亮数据传递给父组件执行删除
   * 3. 调用 onClose 关闭气泡
   */
  const handleDelete = useCallback(() => {
    if (highlight && onDelete) {
      onDelete(highlight); // 将高亮数据传递给父组件处理删除
    }
    onClose(); // 关闭气泡
  }, [highlight, onDelete, onClose]);

  /**
   * 当定位坐标 (x, y) 变化时，更新气泡位置
   *
   * 为什么用 state 而不是直接使用 props？
   * - x/y 是 props，每次父组件重渲染可能传入新值
   * - 使用 state 缓存位置，可以在关闭动画期间保持最后的位置
   */
  useEffect(() => {
    setPopoverPosition({ x, y });
  }, [x, y]);

  /**
   * 点击外部或按 Escape 键关闭气泡
   *
   * 使用文档级事件监听器实现，而不是 backdrop 蒙层：
   * - CSS Custom Highlight 没有对应的 DOM 元素，无法使用 backdrop
   * - 文档级监听可以捕获所有区域的点击
   *
   * 注意：使用 setTimeout 延迟注册 click 监听器，避免打开气泡的
   * 当前点击事件立即触发关闭逻辑。
   */
  useEffect(() => {
    // 如果气泡不可见，不需要注册监听器
    if (!visible) return;

    /**
     * Escape 键处理：按 Escape 键关闭气泡
     * 这是浮层/弹窗的通用关闭快捷键
     */
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };

    /**
     * 点击外部处理：点击气泡外部区域时关闭气泡
     *
     * 例外情况（不关闭）：
     * 1. 点击在气泡内部 → 由气泡自身的按钮处理
     * 2. 点击在高亮元素上（.highlight-rect 或 [data-highlight-id]）
     *    → 让高亮元素的点击处理器处理，避免气泡闪烁
     */
    const handleClickOutside = (e: MouseEvent) => {
      // 检查点击是否在气泡 DOM 内部
      const popoverEl = popoverRef.current;
      if (popoverEl && popoverEl.contains(e.target as Node)) return;

      // 检查点击是否在高亮元素上（让高亮点击处理器处理）
      const target = e.target as HTMLElement;
      if (target && target.closest && target.closest('.highlight-rect, [data-highlight-id]')) {
        return;
      }

      // 点击在外部，关闭气泡
      onClose();
    };

    // 注册 Escape 键监听器（立即生效）
    document.addEventListener('keydown', handleKeyDown);
    // 延迟注册 click 监听器，避免当前打开点击触发关闭
    // setTimeout 0 将回调放入下一个事件循环 tick
    const timer = setTimeout(() => {
      document.addEventListener('click', handleClickOutside, true); // 捕获阶段
    }, 0);

    // 清理函数：移除所有事件监听器和定时器
    return () => {
      clearTimeout(timer);
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('click', handleClickOutside, true);
    };
  }, [visible, onClose]);

  // 如果气泡不可见或没有高亮数据，不渲染
  if (!visible || !highlight) return null;

  /**
   * 气泡定位计算
   *
   * 水平方向：居中于点击位置，但约束在 [120, viewportWidth - 120] 范围内
   * 垂直方向：在点击位置上方 12px（transform: translate(-50%, -100%) 实现上方居中）
   * 最小值 80/120 确保气泡不会被裁切到视口之外
   */
  const popoverX = Math.max(120, Math.min(popoverPosition.x, window.innerWidth - 120));
  const popoverY = Math.max(80, popoverPosition.y - 12);

  // 使用 Portal 将气泡渲染到 document.body
  // 这样可以避免被父容器的 overflow:hidden 裁切
  return createPortal(
    <div
      ref={popoverRef}
      data-highlight-popover="true"  // 数据属性，用于外部点击检测时识别气泡
      style={{
        position: 'fixed',                        // 固定定位，基于视口坐标
        left: popoverX,                           // 水平位置（已约束）
        top: popoverY,                            // 垂直位置（点击位置上方）
        transform: 'translate(-50%, -100%)',      // 居中对齐 + 向上偏移（显示在点击位置上方）
        zIndex: 1000,                             // 高层级，确保在所有内容之上
        minWidth: 160,                            // 最小宽度 160px
        maxWidth: 260,                            // 最大宽度 260px
        background: 'var(--bg-elevated)',         // 背景色（CSS 变量，适配深色模式）
        border: '1px solid var(--border-color)',  // 边框（CSS 变量）
        borderRadius: 8,                          // 8px 圆角
        padding: '8px 12px',                      // 内边距
        boxShadow: 'var(--shadow-md)',            // 阴影（CSS 变量）
        pointerEvents: 'auto',                    // 允许鼠标交互
      }}
    >
      {/* 高亮文本摘要（最多显示 2 行，超出显示省略号） */}
      <div
        style={{
          fontSize: 13,                            // 13px 字体
          color: 'var(--text-secondary)',          // 次要文字颜色
          marginBottom: 8,                         // 底部间距 8px
          overflow: 'hidden',                      // 隐藏溢出内容
          textOverflow: 'ellipsis',                // 文本溢出显示省略号
          display: '-webkit-box',                  // 弹性盒布局（用于多行截断）
          WebkitLineClamp: 2,                      // 最多显示 2 行
          WebkitBoxOrient: 'vertical',             // 垂直方向排列
          lineHeight: 1.4,                         // 1.4 倍行高
        }}
      >
        {highlight.text}
      </div>
      {/* 操作按钮区域 */}
      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        {/* Popconfirm: 二次确认弹出框，包裹删除按钮 */}
        <Popconfirm
          title="确认删除"                         // 确认框标题
          description="确定要删除这条高亮吗？（可通过 Ctrl+Z 撤销）" // 确认框描述
          onConfirm={handleDelete}                 // 确认回调：执行删除
          okText="删除"                            // 确认按钮文字
          cancelText="取消"                        // 取消按钮文字
          okButtonProps={{ danger: true }}         // 确认按钮样式：危险（红色）
        >
          {/* 删除按钮 */}
          <button
            className="highlight-delete-btn"
            style={{
              display: 'inline-flex',              // 行内弹性布局（图标和文字同行）
              alignItems: 'center',                // 垂直居中
              gap: 4,                              // 图标和文字间距 4px
              padding: '2px 8px',                  // 上下 2px，左右 8px 内边距
              fontSize: 12,                        // 12px 字体
              color: 'var(--text-tertiary)',       // 次要文字颜色（CSS 变量）
              background: 'transparent',           // 透明背景
              border: 'none',                      // 无边框
              borderRadius: 4,                     // 4px 圆角
              cursor: 'pointer',                   // 手型光标
              transition: 'color 0.2s',            // 颜色过渡动画 0.2s
            }}
          >
            <DeleteOutlined style={{ fontSize: 12 }} /> {/* 删除图标 */}
            删除
          </button>
        </Popconfirm>
      </div>
    </div>,
    document.body // Portal 目标：渲染到 body 元素
  );
}
