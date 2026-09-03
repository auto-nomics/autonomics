/**
 * JayRead 通用面板拖拽调整宽度 Hook
 *
 * 封装面板拖拽调整宽度的逻辑，支持左/右方向。
 * 基于 useResizableSidebar 的模式泛化而来。
 *
 * 功能说明：
 * 1. 拖拽手柄：用户拖拽手柄时实时更新面板宽度
 * 2. 宽度限制：防止面板过窄或过宽
 * 3. 视觉反馈：拖拽时鼠标指针变为 col-resize，禁止文字选中
 * 4. 方向支持：reverse 模式适用于右侧面板（向右拖拽缩小）
 *
 * @module useResizablePanel
 */

import { useState, useRef, useCallback } from 'react';

/**
 * 通用面板拖拽调整宽度 Hook
 *
 * @param {object} options - Hook 配置选项
 * @param {number} options.defaultWidth - 面板默认宽度（像素），默认 240
 * @param {number} options.minWidth - 面板最小宽度（像素），默认 180
 * @param {number} options.maxWidth - 面板最大宽度（像素），默认 480
 * @param {boolean} options.reverse - 是否反向（右侧面板用），默认 false
 * @returns {object} Hook 返回的状态和方法
 */
export function useResizablePanel({
  defaultWidth = 240,
  minWidth = 180,
  maxWidth = 480,
  reverse = false,
} = {}) {
  const [width, setWidth] = useState(defaultWidth);
  const isResizing = useRef(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(0);

  const handleResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    isResizing.current = true;
    startXRef.current = e.clientX;
    startWidthRef.current = width;

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const handleMouseMove = (moveEvent: MouseEvent) => {
      if (!isResizing.current) return;

      const delta = moveEvent.clientX - startXRef.current;
      const effectiveDelta = reverse ? -delta : delta;
      const newWidth = Math.min(Math.max(startWidthRef.current + effectiveDelta, minWidth), maxWidth);

      setWidth(newWidth);
    };

    const handleMouseUp = () => {
      isResizing.current = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
  }, [width, minWidth, maxWidth, reverse]);

  return { width, handleResizeStart, isResizing };
}

export default useResizablePanel;
