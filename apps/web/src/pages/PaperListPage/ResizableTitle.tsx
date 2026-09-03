/**
 * ResizableTitle - 可拖拽调整宽度的表格头组件
 *
 * 本文件实现了 Ant Design Table 的可拖拽列宽功能，包括：
 * - 在表头单元格右边缘显示拖拽手柄
 * - 鼠标按下后开始拖拽，实时计算列宽
 * - 最小宽度限制（40px），防止列过窄
 * - 拖拽过程中改变鼠标光标样式
 * - 拖拽完成后触发 onResize 回调，持久化列宽配置
 *
 * 为什么需要自定义表头组件？
 * - Ant Design Table 默认不支持列宽拖拽
 * - 需要通过 components.header.cell 属性替换默认渲染
 * - 配合 table-layout:fixed CSS 实现精确列宽控制
 *
 * 实现原理：
 * 1. 在 th 元素内添加一个绝对定位的 div 作为拖拽手柄
 * 2. 手柄位于右边缘，宽度 7px，光标为 col-resize
 * 3. onMouseDown 开始拖拽，记录起始位置和列宽
 * 4. document 上监听 mouseMove 和 mouseUp 事件
 * 5. mouseMove 时计算新的列宽并调用 onResize
 * 6. mouseUp 时清理事件监听器
 */

import { useRef } from 'react'; // 导入 React useRef 钩子，用于存储拖拽手柄引用

/**
 * 可拖拽调整宽度的表头单元格组件
 *
 * @param {Object} props - Ant Design Table 传递的属性
 * @param {Function} props.onResize - 列宽变化回调，签名：(newWidth: number) => void
 * @param {number} props.width - 当前列宽度（像素）
 * @param {Function} props.onClick - 表头点击事件（如排序）
 * @param {Object} props.style - 自定义样式
 * @param {string} props.className - CSS 类名
 * @param {React.ReactNode} props.children - 子元素（列标题文本）
 * @returns {JSX.Element} 返回自定义的 th 元素
 */
export default function ResizableTitle(props: any) {
  // 解构属性，分离出拖拽相关属性和其他 th 属性
  const { onResize, width, onClick, style, className, ...restProps }: any = props;

  // 创建拖拽手柄的 ref，用于在点击事件中判断是否点击在手柄上
  const resizeRef = useRef(null); // ref：拖拽手柄元素引用

  // 如果没有指定宽度，返回默认 th 元素（不可拖拽）
  if (!width) {
    return <th {...restProps} />; // 返回默认表头单元格
  }

  /**
   * 鼠标按下事件处理函数，开始拖拽
   *
   * @param {React.MouseEvent} e - 鼠标事件对象
   */
  const handleMouseDown = (e: any) => {
    // 阻止默认行为和事件冒泡
    e.preventDefault(); // 防止文本选择
    e.stopPropagation(); // 防止触发表头点击（如排序）

    // 记录拖拽起始位置（鼠标 X 坐标）
    const startX = e.pageX; // 起始鼠标位置

    // 记录起始列宽（直接使用逻辑宽度，不使用 offsetWidth）
    // 为什么不使用 offsetWidth？
    // - CSS 中设置了 table-layout:fixed 且不设 width:100%
    // - 这确保列的实际渲染宽度等于指定的逻辑宽度
    // - 不存在比例扩展，所以可以直接使用 width prop
    const startWidth = width; // 起始列宽

    /**
     * 鼠标移动事件处理函数，计算并更新列宽
     *
     * @param {MouseEvent} ev - 鼠标移动事件
     */
    const handleMouseMove = (ev: any) => {
      // 计算鼠标移动的距离（当前 X - 起始 X）
      const delta = ev.pageX - startX; // 拖拽距离（正数为向右拖，负数为向左拖）

      // 计算新的列宽，最小宽度限制为 40px
      // Math.max(40, startWidth + delta) 确保列宽不小于 40px
      const newWidth = Math.max(40, startWidth + delta); // 最小宽度 40px

      // 调用父组件传入的 onResize 回调，更新列宽状态
      onResize?.(newWidth); // 如果提供了回调函数，则调用
    };

    /**
     * 鼠标松开事件处理函数，结束拖拽
     * 清理事件监听器和鼠标样式
     */
    const handleMouseUp = () => {
      // 移除 mouseMove 事件监听器
      document.removeEventListener('mousemove', handleMouseMove); // 停止跟踪鼠标移动

      // 移除 mouseUp 事件监听器
      document.removeEventListener('mouseup', handleMouseUp); // 停止跟踪鼠标松开

      // 恢复鼠标光标样式
      document.body.style.cursor = ''; // 清除全局光标样式

      // 恢复文本选择功能
      document.body.style.userSelect = ''; // 清除不可选择样式
    };

    // 在 document 上注册鼠标移动事件监听器（确保拖拽出表格区域仍能响应）
    document.addEventListener('mousemove', handleMouseMove);

    // 在 document 上注册鼠标松开事件监听器
    document.addEventListener('mouseup', handleMouseUp);

    // 设置全局鼠标光标为 col-resize（调整列宽样式）
    document.body.style.cursor = 'col-resize';

    // 禁止文本选择（拖拽过程中不应选中文字）
    document.body.style.userSelect = 'none';
  };

  /**
   * 表头单元格点击事件处理函数
   *
   * @param {React.MouseEvent} e - 鼠标事件对象
   */
  const handleClick = (e: any) => {
    // 如果点击的是拖拽手柄，不触发 onClick（如排序）
    // e.target === resizeRef.current 判断点击目标是否是手柄元素
    if (e.target === resizeRef.current) {
      return; // 点击手柄时不处理
    }
    // 否则调用父组件传入的 onClick 回调
    onClick?.(e); // 如果提供了 onClick，则调用
  };

  // 返回自定义的 th 元素
  return (
    <th
      {...restProps} // 展开原始 props（包含 dataIndex, key, align 等 Ant Design 属性）
      style={{
        ...style, // 保留父组件传入的自定义样式
        position: 'relative', // 相对定位，为拖拽手柄提供定位上下文
        overflow: 'visible',  // 可见溢出，确保拖拽手柄显示在表格边框外
      }}
      className={className} // 保留父组件传入的 CSS 类名
      onClick={handleClick} // 点击事件处理
    >
      {/* 渲染子元素（列标题文本，如"标题"、"作者"等） */}
      {restProps.children}

      {/* 拖拽手柄：绝对定位在 th 右边缘 */}
      <div
        ref={resizeRef} // 存储 ref，用于判断点击目标
        onMouseDown={handleMouseDown} // 鼠标按下事件：开始拖拽
        style={{
          position: 'absolute',  // 绝对定位
          right: -2,             // 距离右边缘 -2px（一半在表格内，一半在外，增大点击区域）
          bottom: 0,             // 底部对齐
          width: 7,              // 手柄宽度 7px（足够用户点击，不过大）
          height: '100%',        // 占满单元格高度
          cursor: 'col-resize',  // 鼠标悬停时显示调整列宽光标（←→）
          zIndex: 1,             // 提升层级，确保在其他内容之上
        }}
        // 点击手柄时阻止事件冒泡，避免触发表头点击
        onClick={(e) => e.stopPropagation()}
      />
    </th>
  );
}

/**
 * 使用示例：
 *
 * import ResizableTitle from './pages/components/ResizableTitle';
 *
 * <Table
 *   components={{
 *     header: {
 *       cell: ResizableTitle, // 替换默认表头单元格
 *     },
 *   }}
 *   columns={[
 *     {
 *       title: '标题',
 *       dataIndex: 'title',
 *       width: 300,
 *       onHeaderCell: () => ({
 *         width: 300,
 *         onResize: (newWidth) => {
 *           // 更新列宽状态并持久化到 localStorage
 *           setColumnWidths(prev => ({
 *             ...prev,
 *             title: newWidth,
 *           }));
 *         },
 *       }),
 *     },
 *   ]}
 * />
 */
