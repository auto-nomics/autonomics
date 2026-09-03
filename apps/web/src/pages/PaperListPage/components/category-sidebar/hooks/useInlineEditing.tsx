/**
 * JayRead 分类侧边栏内联编辑 Hook
 *
 * 封装分类树的内联编辑（新建/重命名）逻辑。
 *
 * 功能说明：
 * 1. 内联创建：在分类节点下显示输入框，创建子分类
 * 2. 内联重命名：将分类标题替换为输入框，修改分类名称
 * 3. 自动聚焦：编辑状态激活时自动聚焦输入框
 * 4. 键盘操作：Enter 确认，Escape 取消
 * 5. 失焦提交：点击输入框外部自动提交
 *
 * @module useInlineEditing
 */

import { useState, useCallback, useEffect, useRef } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 App 组件（用于获取 message 实例）
import { createCategory, renameCategory } from '../../../../../services/categoriesApi'; // 导入分类 API

/** 编辑节点状态 */
interface EditingNode {
  type: 'create' | 'rename';
  parentId?: string | null;
  categoryId?: string;
  value: string;
}

/** 分类数据 */
interface CategoryData {
  id: string;
  name: string;
  paper_count: number;
}

/** 额外参数 */
interface EditExtra {
  parentId?: string | null;
  categoryId?: string;
}

/** useInlineEditing 参数 */
interface UseInlineEditingParams {
  onEditComplete?: () => Promise<void>;
}

/**
 * 分类侧边栏内联编辑 Hook
 *
 * @param {object} options - Hook 配置选项
 * @param {Function} options.onEditComplete - 编辑完成后的回调函数，用于刷新分类树
 * @returns {object} Hook 返回的状态和方法
 * @returns {object|null} returns.editingNode - 当前正在编辑的节点状态
 * @returns {React.RefObject} returns.inputRef - 输入框的引用，用于自动聚焦
 * @returns {Function} returns.startCreate - 开始创建新分类
 * @returns {Function} returns.startRename - 开始重命名分类
 * @returns {Function} returns.handleInputKeyDown - 输入框按键事件处理
 * @returns {Function} returns.handleInputBlur - 输入框失焦事件处理
 * @returns {Function} returns.renderEnhancedTitle - 渲染增强的树节点标题（支持内联编辑）
 */
export function useInlineEditing({ onEditComplete }: UseInlineEditingParams) {
  const { message } = App.useApp();
  // ========== 状态定义 ==========

  // 内联编辑状态：用于新建和重命名分类的输入框
  // null 表示没有编辑中的节点
  // 对象结构：{ type: 'create' | 'rename', parentId?, nodeId?, value }
  const [editingNode, setEditingNode] = useState<EditingNode | null>(null); // 当前正在编辑的节点

  // 新建分类输入框的引用，用于自动聚焦
  const inputRef = useRef<HTMLInputElement | null>(null); // Input 组件的引用

  // ========== 自动聚焦 ==========

  /**
   * 当编辑状态变化时，自动聚焦输入框
   *
   * 使用 setTimeout 延迟聚焦，确保 Input 组件已完成渲染。
   * 如果直接调用 focus()，可能因为组件未挂载而失败。
   */
  useEffect(() => {
    // 如果正在编辑且输入框引用存在，自动聚焦
    if (editingNode && inputRef.current) { // 检查编辑状态和引用
      // 延迟聚焦，确保 Input 组件已完成渲染
      setTimeout(() => { // 使用 setTimeout 延迟执行
        inputRef.current?.focus(); // 调用 Input 的 focus 方法
      }, 50); // 延迟 50 毫秒，确保 DOM 已更新
    }
  }, [editingNode]); // 依赖编辑状态

  // ========== 创建分类 ==========

  /**
   * 开始创建新分类
   *
   * 设置编辑状态为创建模式，父分类 ID 由调用者指定。
   *
   * @param {number|null} parentId - 父分类 ID，null 表示创建顶级分类
   */
  const startCreate = useCallback((parentId = null) => {
    setEditingNode({ // 设置编辑状态为创建模式
      type: 'create', // 编辑类型为创建
      parentId, // 父分类 ID
      value: '', // 初始值为空
    });
  }, []); // 空依赖

  /**
   * 处理创建分类（确认提交）
   *
   * 从内联输入框获取分类名称，调用 API 创建新分类。
   * 创建成功后重新加载分类树，并清除编辑状态。
   *
   * @param {string} name - 新分类名称
   * @param {number|null} parentId - 父分类 ID，null 表示顶级分类
   */
  const handleCreate = useCallback(async (name: any, parentId: any) => {
    // 校验分类名称不能为空
    if (!name || !name.trim()) { // 检查名称是否为空或纯空格
      // 清除编辑状态
      setEditingNode(null); // 取消编辑
      return; // 直接返回
    }

    try { // 开始 try-catch
      // 调用 API 创建新分类
      await createCategory(name.trim(), parentId); // 异步调用创建分类接口，传入名称和父分类 ID
      // 显示成功提示
      message.success('分类已创建'); // 弹出成功提示
      // 清除编辑状态
      setEditingNode(null); // 取消编辑
      // 通知父组件：编辑完成，需要刷新分类树
      if (onEditComplete) { // 检查回调是否存在
        await onEditComplete(); // 调用父组件的刷新函数
      }
    } catch (err) { // 捕获创建错误
      // 显示错误提示
      message.error('创建分类失败: ' + (err as any).message); // 弹出错误提示
    }
  }, [onEditComplete]); // 依赖编辑完成回调

  // ========== 重命名分类 ==========

  /**
   * 开始重命名分类
   *
   * 设置编辑状态为重命名模式，分类 ID 由调用者指定。
   *
   * @param {number} categoryId - 要重命名的分类 ID
   * @param {string} currentName - 当前分类名称（作为输入框初始值）
   */
  const startRename = useCallback((categoryId: any, currentName: any) => {
    setEditingNode({ // 设置编辑状态为重命名模式
      type: 'rename', // 编辑类型为重命名
      categoryId, // 要重命名的分类 ID
      value: currentName, // 初始值为当前名称
    });
  }, []); // 空依赖

  /**
   * 处理重命名分类（确认提交）
   *
   * 从内联输入框获取新名称，调用 API 更新分类名称。
   * 重命名成功后重新加载分类树，并清除编辑状态。
   *
   * @param {number} categoryId - 要重命名的分类 ID
   * @param {string} newName - 新分类名称
   */
  const handleRename = useCallback(async (categoryId: any, newName: any) => {
    // 校验新名称不能为空
    if (!newName || !newName.trim()) { // 检查名称是否为空或纯空格
      // 清除编辑状态
      setEditingNode(null); // 取消编辑
      return; // 直接返回
    }

    try { // 开始 try-catch
      // 调用 API 重命名分类
      await renameCategory(categoryId, newName.trim()); // 异步调用重命名接口
      // 显示成功提示
      message.success('分类已重命名'); // 弹出成功提示
      // 清除编辑状态
      setEditingNode(null); // 取消编辑
      // 通知父组件：编辑完成，需要刷新分类树
      if (onEditComplete) { // 检查回调是否存在
        await onEditComplete(); // 调用父组件的刷新函数
      }
    } catch (err) { // 捕获重命名错误
      // 显示错误提示
      message.error('重命名失败: ' + (err as any).message); // 弹出错误提示
    }
  }, [onEditComplete]); // 依赖编辑完成回调

  // ========== 事件处理 ==========

  /**
   * 处理内联输入框的按键事件
   *
   * Enter 键：确认提交（创建或重命名）
   * Escape 键：取消编辑
   *
   * @param {KeyboardEvent} e - 键盘事件对象
   * @param {string} type - 编辑类型：'create' 或 'rename'
   * @param {object} extra - 额外参数 { parentId, categoryId }
   */
  const handleInputKeyDown = useCallback((e: any, type: any, extra: any) => {
    // 按 Enter 键：确认提交
    if (e.key === 'Enter') { // 检查是否按下 Enter
      e.preventDefault(); // 阻止默认行为（防止表单提交）
      const value = e.target.value; // 获取输入框的值

      if (type === 'create') { // 如果是创建操作
        handleCreate(value, extra.parentId); // 调用创建处理函数
      } else if (type === 'rename') { // 如果是重命名操作
        handleRename(extra.categoryId, value); // 调用重命名处理函数
      }
    }

    // 按 Escape 键：取消编辑
    if (e.key === 'Escape') { // 检查是否按下 Escape
      e.preventDefault(); // 阻止默认行为
      setEditingNode(null); // 清除编辑状态，隐藏输入框
    }
  }, [handleCreate, handleRename]); // 依赖创建和重命名处理函数

  /**
   * 处理内联输入框失去焦点事件
   *
   * 失去焦点时自动提交（等同于按 Enter 键）。
   * 这样用户点击输入框外的区域也能完成编辑。
   *
   * @param {FocusEvent} e - 焦点事件对象
   * @param {string} type - 编辑类型：'create' 或 'rename'
   * @param {object} extra - 额外参数 { parentId, categoryId }
   */
  const handleInputBlur = useCallback((e: any, type: any, extra: any) => {
    const value = e.target.value; // 获取输入框的值

    if (type === 'create') { // 如果是创建操作
      handleCreate(value, extra.parentId); // 调用创建处理函数
    } else if (type === 'rename') { // 如果是重命名操作
      handleRename(extra.categoryId, value); // 调用重命名处理函数
    }
  }, [handleCreate, handleRename]); // 依赖创建和重命名处理函数

  // ========== 渲染增强标题 ==========

  /**
   * 渲染增强的树节点标题
   *
   * 根据当前的编辑状态，决定渲染普通标题还是内联输入框。
   * 内联输入框用于新建分类和重命名分类操作。
   *
   * 这是一个高阶函数，调用后返回一个 JSX 渲染函数。
   * 调用者需要传入 renderCategoryTitle 函数和 Input 组件。
   *
   * @param {Function} renderCategoryTitle - 渲染普通分类标题的函数
   * @param {React.Component} InputComponent - Input 组件（从 antd 导入）
   * @returns {Function} 返回一个渲染函数，接收 categoryData 参数
   */
  const createEnhancedTitleRenderer = useCallback((renderCategoryTitle: any, InputComponent: any) => {
    /**
     * 渲染增强的树节点标题
     *
     * @param {object} categoryData - 分类数据 { id, name, parent_id, paper_count, ... }
     * @returns {JSX.Element} 树节点标题元素
     */
    return (categoryData: any) => {
      // 检查当前节点是否正在编辑中
      const isCreating = editingNode?.type === 'create' && editingNode?.parentId === categoryData.id; // 是否正在此节点下创建子分类
      const isRenaming = editingNode?.type === 'rename' && editingNode?.categoryId === categoryData.id; // 是否正在重命名此节点

      // 如果正在创建子分类，在此节点的标题后渲染内联输入框
      if (isCreating) { // 如果正在此节点下创建子分类
        return (
          <span style={{ display: 'flex', flexDirection: 'column' }}> {/* 纵向布局容器 */}
            {/* 原始分类标题 */}
            {renderCategoryTitle(categoryData.name, categoryData.paper_count)} {/* 渲染正常标题 */}
            {/* 内联创建输入框 */}
            <InputComponent
              ref={inputRef} // 绑定输入框引用（用于自动聚焦）
              size="small" // 小号输入框
              placeholder="输入分类名称" // 占位提示文字
              style={{ marginTop: 4, fontSize: 12 }} // 样式：上间距 + 小字号
              // 按键事件：Enter 确认，Escape 取消
              onKeyDown={(e: any) => handleInputKeyDown(e, 'create', { parentId: categoryData.id })} // 绑定按键事件
              // 失焦事件：自动提交
              onBlur={(e: any) => handleInputBlur(e, 'create', { parentId: categoryData.id })} // 绑定失焦事件
              // 阻止点击冒泡，避免触发树节点的选中事件
              onClick={(e: any) => e.stopPropagation()} // 阻止冒泡
            />
          </span>
        );
      }

      // 如果正在重命名此节点，渲染内联输入框（初始值为当前名称）
      if (isRenaming) { // 如果正在重命名此节点
        return (
          <InputComponent
            ref={inputRef} // 绑定输入框引用
            size="small" // 小号输入框
            defaultValue={categoryData.name} // 默认值为当前分类名称
            style={{ fontSize: 12 }} // 样式：小字号
            // 按键事件：Enter 确认，Escape 取消
            onKeyDown={(e: any) => handleInputKeyDown(e, 'rename', { categoryId: categoryData.id })} // 绑定按键事件
            // 失焦事件：自动提交
            onBlur={(e: any) => handleInputBlur(e, 'rename', { categoryId: categoryData.id })} // 绑定失焦事件
            // 阻止点击冒泡
            onClick={(e: any) => e.stopPropagation()} // 阻止冒泡
          />
        );
      }

      // 默认：渲染普通分类标题（名称 + 论文数量）
      return renderCategoryTitle(categoryData.name, categoryData.paper_count); // 渲染普通标题
    };
  }, [editingNode, handleInputKeyDown, handleInputBlur]); // 依赖编辑状态和事件处理函数

  // ========== 渲染根级别创建输入框 ==========

  /**
   * 渲染根级别的创建输入框
   *
   * 当 editingNode 的 parentId 为 null 时，表示正在创建顶级分类。
   * 此时在分类树底部渲染一个内联输入框。
   *
   * @param {React.Component} InputComponent - Input 组件（从 antd 导入）
   * @returns {JSX.Element|null} 渲染输入框或 null
   */
  const renderRootCreateInput = useCallback((InputComponent: any) => {
    // 如果不是在根级别创建分类，不渲染
    if (!editingNode || editingNode.type !== 'create' || editingNode.parentId !== null) { // 检查条件
      return null; // 不渲染
    }

    // 渲染内联创建输入框
    return (
      <div style={{ padding: '4px 12px' }}> {/* 容器，添加内边距 */}
        <InputComponent
          ref={inputRef} // 绑定输入框引用
          size="small" // 小号输入框
          placeholder="输入分类名称" // 占位提示文字
          // 按键事件：Enter 确认，Escape 取消
          onKeyDown={(e: any) => handleInputKeyDown(e, 'create', { parentId: null })} // 绑定按键事件
          // 失焦事件：自动提交
          onBlur={(e: any) => handleInputBlur(e, 'create', { parentId: null })} // 绑定失焦事件
        />
      </div>
    );
  }, [editingNode, handleInputKeyDown, handleInputBlur]); // 依赖编辑状态和事件处理函数

  // ========== 返回值 ==========

  // 返回状态和方法，供组件使用
  return {
    editingNode, // 当前正在编辑的节点状态
    inputRef, // 输入框的引用
    startCreate, // 开始创建新分类
    startRename, // 开始重命名分类
    handleInputKeyDown, // 输入框按键事件处理
    handleInputBlur, // 输入框失焦事件处理
    createEnhancedTitleRenderer, // 创建增强标题渲染器的函数
    renderRootCreateInput, // 渲染根级别创建输入框的函数
    setEditingNode, // 直接设置编辑节点的方法（用于取消编辑）
  };
}

/**
 * 默认导出：useInlineEditing Hook
 *
 * 使用示例：
 * ```jsx
 * const { editingNode, inputRef, startCreate, startRename, createEnhancedTitleRenderer, renderRootCreateInput } = useInlineEditing({
 *   onEditComplete: async () => {
 *     await loadCategories(); // 刷新分类树
 *   },
 * });
 *
 * // 在 JSX 中使用：
 * // const renderTitle = createEnhancedTitleRenderer(renderCategoryTitle, Input);
 * // <Tree treeData={treeData.map(node => ({ ...node, title: renderTitle(node.raw) }))} />
 * // {renderRootCreateInput(Input)}
 * ```
 */
export default useInlineEditing;
