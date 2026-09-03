/**
 * useResizable Hook
 *
 * 通用的面板宽度拖拽调整 hook
 *
 * 支持左侧面板（如导览栏）和右侧面板（如聊天面板）的宽度调整。
 * 左侧面板：鼠标向右移动时宽度增大（delta 为正）
 * 右侧面板：鼠标向左移动时宽度增大（delta 为负，因为手柄在面板左侧）
 */
import { useCallback, useRef } from 'react';

interface UseResizableParams {
  currentWidth: number;
  minWidth: number;
  maxWidth: number;
  direction?: 'left' | 'right';
  onWidthChange: (width: number) => void;
  onDragStart?: (key: string | null) => void;
  draggingKey?: string;
}

export default function useResizable({
  currentWidth,
  minWidth,
  maxWidth,
  direction = 'left',
  onWidthChange,
  onDragStart,
  draggingKey,
}: UseResizableParams) {
  // Refs to track current handlers for proper cleanup
  const handlersRef = useRef<{ handleMouseMove: ((e: any) => void) | null; handleMouseUp: (() => void) | null }>({ handleMouseMove: null, handleMouseUp: null });

  /**
   * 处理拖拽开始
   *
   * 当用户在拖拽手柄上按下鼠标时触发：
   * 1. 清理之前可能残留的监听器（防止内存泄漏）
   * 2. 记录鼠标起始 X 坐标和当前宽度
   * 3. 调用 onDragStart 回调（如果提供了）
   * 4. 在 document 上注册 mousemove 和 mouseup 监听器
   */
  const handleResizeStart = useCallback((e: any) => {
    e.preventDefault();

    // Clean up any previous listeners before adding new ones
    if (handlersRef.current.handleMouseMove) {
      document.removeEventListener('mousemove', handlersRef.current.handleMouseMove);
    }
    if (handlersRef.current.handleMouseUp) {
      document.removeEventListener('mouseup', handlersRef.current.handleMouseUp);
    }

    const startX = e.clientX;
    const startWidth = currentWidth;

    if (onDragStart && draggingKey) {
      onDragStart(draggingKey);
    }

    const handleMouseMove = (e: any) => {
      const delta = e.clientX - startX;
      const newWidth = direction === 'left'
        ? startWidth + delta
        : startWidth - delta;
      const clampedWidth = Math.min(maxWidth, Math.max(minWidth, newWidth));
      onWidthChange(clampedWidth);
    };

    const handleMouseUp = () => {
      // Clear drag state
      if (onDragStart) {
        onDragStart(null);
      }
      // Remove listeners
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      // Clear refs
      handlersRef.current.handleMouseMove = null;
      handlersRef.current.handleMouseUp = null;
    };

    // Store handler refs for cleanup
    handlersRef.current.handleMouseMove = handleMouseMove;
    handlersRef.current.handleMouseUp = handleMouseUp;

    // Add listeners
    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
  }, [currentWidth, minWidth, maxWidth, direction, onWidthChange, onDragStart, draggingKey]);

  return handleResizeStart;
}
