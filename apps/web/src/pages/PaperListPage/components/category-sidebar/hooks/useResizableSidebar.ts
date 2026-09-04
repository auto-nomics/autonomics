/**
 * Autonomics 分类侧边栏拖拽调整宽度 Hook
 *
 * 封装侧边栏拖拽调整宽度的逻辑。
 *
 * 功能说明：
 * 1. 拖拽手柄：在侧边栏右边缘显示拖拽手柄
 * 2. 宽度调整：用户拖拽手柄时实时更新侧边栏宽度
 * 3. 宽度限制：最小 180px，最大 480px，防止侧边栏过窄或过宽
 * 4. 视觉反馈：拖拽时鼠标指针变为 col-resize，禁止文字选中
 * 5. 状态管理：使用 ref 存储拖拽状态，避免频繁更新组件
 *
 * @module useResizableSidebar
 */

import { useState, useRef, useCallback } from 'react'; // 导入 React 核心钩子

/**
 * 分类侧边栏拖拽调整宽度 Hook
 *
 * @param {object} options - Hook 配置选项
 * @param {number} options.defaultWidth - 侧边栏默认宽度（像素），默认 240
 * @param {number} options.minWidth - 侧边栏最小宽度（像素），默认 180
 * @param {number} options.maxWidth - 侧边栏最大宽度（像素），默认 480
 * @param {number} [options.externalMaxWidth] - 外部约束的最大宽度（像素），可选
 *   当外部环境（如筛选模式的面板宽度）需要限制侧边栏最大宽度时传入。
 *   如果传入，会取此值与 maxWidth 的较小值作为实际最大宽度约束。
 * @returns {object} Hook 返回的状态和方法
 * @returns {number} returns.sidebarWidth - 当前侧边栏宽度
 * @returns {Function} returns.handleResizeStart - 拖拽开始的处理函数（绑定到手柄的 onMouseDown）
 * @returns {React.MutableRefObject} returns.isResizing - 是否正在拖拽调整宽度的 ref
 */
export function useResizableSidebar({
  defaultWidth = 240, // 默认宽度：240px
  minWidth = 180, // 最小宽度：180px
  maxWidth = 480, // 最大宽度：480px
  externalMaxWidth, // 外部约束的最大宽度（可选），筛选模式下由左面板宽度决定
}: {
  defaultWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  externalMaxWidth?: number;
} = {}) {
  // ========== 状态定义 ==========

  // 实际最大宽度约束：取 maxWidth 和 externalMaxWidth 的较小值
  // 当筛选模式限制了左面板宽度时，externalMaxWidth 会传入左面板宽度，
  // 防止侧边栏在筛选模式下被拖拽得比左面板还宽
  const effectiveMaxWidth = externalMaxWidth != null // 检查是否传入了外部约束
    ? Math.min(maxWidth, externalMaxWidth) // 取两者较小值
    : maxWidth; // 未传入外部约束时使用默认 maxWidth

  // 侧边栏宽度状态：支持拖拽调整
  // 初始值也受 effectiveMaxWidth 约束，防止初始宽度超过外部限制
  const [sidebarWidth, setSidebarWidth] = useState(
    () => Math.min(defaultWidth, effectiveMaxWidth) // 确保初始宽度不超过最大约束
  ); // 默认宽度 240px（受外部约束限制）

  // ========== Ref 定义 ==========

  // 是否正在拖拽调整宽度：使用 ref 而非 state，避免频繁更新组件
  const isResizing = useRef(false); // 是否正在拖拽调整宽度

  // 拖拽起始 X 坐标：记录鼠标按下时的水平位置
  const startXRef = useRef(0); // 拖拽起始 X 坐标

  // 拖拽起始宽度：记录鼠标按下时的侧边栏宽度
  const startWidthRef = useRef(0); // 拖拽起始宽度

  // ========== 拖拽处理 ==========

  /**
   * 处理拖拽手柄的 mousedown 事件
   *
   * 记录起始位置和宽度，绑定全局 mousemove/mouseup 事件。
   *
   * 为什么要绑定全局事件？
   * - 用户拖拽时鼠标可能移出手柄区域
   * - 绑定在 document 上确保拖拽不会中断
   * - mouseup 时清理事件监听器，防止内存泄漏
   *
   * @param {MouseEvent} e - 鼠标按下事件对象
   */
  const handleResizeStart = useCallback((e: any) => {
    e.preventDefault(); // 阻止默认行为（如文字选中、拖拽内容等）

    // 标记开始拖拽
    isResizing.current = true; // 设置拖拽状态为 true

    // 记录起始位置和宽度
    startXRef.current = e.clientX; // 记录鼠标按下的 X 坐标
    startWidthRef.current = sidebarWidth; // 记录当前侧边栏宽度

    // 添加全局样式：拖拽时隐藏鼠标指针的选中效果
    document.body.style.cursor = 'col-resize'; // 设置鼠标指针为水平调整图标
    document.body.style.userSelect = 'none'; // 禁止文字选中，防止拖拽时选中文本

    /**
     * 处理鼠标移动事件
     *
     * 根据鼠标移动的距离计算新的侧边栏宽度，
     * 并限制在最小和最大宽度之间。
     *
     * @param {MouseEvent} moveEvent - 鼠标移动事件对象
     */
    const handleMouseMove = (moveEvent: any) => {
      // 如果不在拖拽状态，不处理
      if (!isResizing.current) return; // 安全检查：防止事件未清理

      // 计算鼠标移动的距离（当前 X 坐标 - 起始 X 坐标）
      const delta = moveEvent.clientX - startXRef.current; // 移动距离，正数表示向右拖拽（变宽），负数表示向左拖拽（变窄）

      // 计算新的宽度：起始宽度 + 移动距离
      // 使用 Math.min 和 Math.max 限制在最小和最大宽度之间
      const newWidth = Math.min(Math.max(startWidthRef.current + delta, minWidth), effectiveMaxWidth); // 限制宽度范围（使用外部约束后的最大宽度）

      // 更新侧边栏宽度状态
      setSidebarWidth(newWidth); // 触发组件重新渲染，更新侧边栏宽度
    };

    /**
     * 处理鼠标松开事件
     *
     * 清理拖拽状态、全局样式和事件监听器。
     */
    const handleMouseUp = () => {
      // 重置拖拽状态
      isResizing.current = false; // 设置拖拽状态为 false

      // 恢复全局样式
      document.body.style.cursor = ''; // 恢复鼠标指针为默认样式
      document.body.style.userSelect = ''; // 恢复文字选中功能

      // 清理事件监听器，防止内存泄漏
      document.removeEventListener('mousemove', handleMouseMove); // 移除鼠标移动监听器
      document.removeEventListener('mouseup', handleMouseUp); // 移除鼠标松开监听器
    };

    // 绑定全局事件监听器
    document.addEventListener('mousemove', handleMouseMove); // 监听鼠标移动，实时更新宽度
    document.addEventListener('mouseup', handleMouseUp); // 监听鼠标松开，清理拖拽状态
  }, [sidebarWidth, minWidth, effectiveMaxWidth]); // 依赖项：宽度配置（使用 effectiveMaxWidth 替代 maxWidth，支持外部约束）

  // ========== 返回值 ==========

  // 返回状态和方法，供组件使用
  return {
    sidebarWidth, // 当前侧边栏宽度
    handleResizeStart, // 拖拽开始的处理函数（绑定到手柄的 onMouseDown）
    isResizing, // 是否正在拖拽调整宽度的 ref（供外部检查）
  };
}

/**
 * 默认导出：useResizableSidebar Hook
 *
 * 使用示例：
 * ```jsx
 * const { sidebarWidth, handleResizeStart } = useResizableSidebar({
 *   defaultWidth: 240,
 *   minWidth: 180,
 *   maxWidth: 480,
 * });
 *
 * // 在 JSX 中使用：
 * // <div style={{ width: sidebarWidth }}>
 * //   <div onMouseDown={handleResizeStart} style={{ cursor: 'col-resize' }} />
 * // </div>
 * ```
 */
export default useResizableSidebar;
