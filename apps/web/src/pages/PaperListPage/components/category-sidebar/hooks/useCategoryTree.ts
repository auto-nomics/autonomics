/**
 * JayRead 分类树数据加载 Hook
 *
 * 封装分类树的数据获取、状态管理和树形结构构建逻辑。
 *
 * 功能说明：
 * 1. 加载分类树：从后端获取扁平列表，自动构建树形结构
 * 2. 论文统计：获取论文总数和未分类论文数
 * 3. 自动展开：加载后默认展开所有分类节点
 * 4. 数据刷新：提供刷新方法，供父组件在数据变更后调用
 * 5. 虚拟节点：生成"全部论文"和"未分类"虚拟节点
 *
 * @module useCategoryTree
 */

import { useState, useEffect, useCallback, useImperativeHandle, useMemo } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 App 组件（用于获取 message 实例）
import { getCategoryTree } from '../../../../../services/categoriesApi'; // 导入分类树 API

/**
 * 将扁平的分类列表构建为树形数据结构
 *
 * 为什么需要这个函数？
 * - 后端返回的是扁平列表（每项包含 parent_id 字段）
 * - Ant Design Tree 组件需要嵌套的树形数据结构（children 数组）
 * - 前端根据 parent_id 递归组织层级关系
 *
 * 算法步骤：
 * 1. 创建 id → node 的映射表（O(n) 遍历）
 * 2. 遍历所有节点，将每个节点挂载到其父节点的 children 下
 * 3. 没有父节点的就是根节点
 *
 * @param {Array} flatList - 后端返回的扁平分类列表
 * @param {Function} renderCategoryTitle - 渲染分类标题的函数
 * @returns {Array} 树形数据结构，每项包含 key, title, icon, children
 */
function buildTreeData(flatList: any, renderCategoryTitle: any) {
  // 第一步：创建 id → 节点对象的映射表，方便后续快速查找父节点
  const map: Record<string, any> = {}; // id 到节点的映射字典

  flatList.forEach((cat: any) => { // 遍历扁平列表中的每个分类
    // 将每个分类转换为 Tree 组件需要的节点格式
    map[cat.id] = { // 以分类 ID 为键存储节点对象
      key: String(cat.id), // Tree 组件要求 key 为字符串类型
      title: renderCategoryTitle(cat.name, cat.paper_count), // 分类标题（包含名称和论文数量）
      // 存储原始分类数据，供右键菜单等操作使用
      raw: { ...cat }, // 复制原始数据（聚合时会修改 paper_count）
      children: [], // 子节点数组，后续会填充
    };
  });

  // 第二步：将每个节点挂载到其父节点的 children 下
  const roots: any[] = []; // 根节点列表（没有父节点的分类）

  flatList.forEach((cat: any) => { // 再次遍历扁平列表
    if (cat.parent_id && map[cat.parent_id]) { // 如果有父节点且父节点存在于映射表中
      // 将当前节点添加到父节点的 children 数组中
      map[cat.parent_id].children.push(map[cat.id]); // 挂载到父节点下
    } else {
      // 没有父节点（或父节点已被删除），作为根节点
      roots.push(map[cat.id]); // 添加到根节点列表
    }
  });

  // 返回根节点列表（包含嵌套的子节点）
  return roots;
}

/**
 * 分类树数据加载 Hook
 *
 * @param {object} options - Hook 配置选项
 * @param {Function} options.renderCategoryTitle - 渲染分类标题的函数，接收名称和数量参数
 * @returns {object} Hook 返回的状态和方法
 * @returns {Array} returns.categories - 所有分类的扁平列表
 * @returns {number} returns.totalCount - 论文总数
 * @returns {number} returns.uncategorizedCount - 未分类论文数
 * @returns {boolean} returns.loading - 加载状态
 * @returns {Array} returns.expandedKeys - 展开的节点 key 数组
 * @returns {Function} returns.loadCategories - 加载分类数据的函数
 * @returns {Function} returns.setExpandedKeys - 设置展开节点的方法
 */
export function useCategoryTree({ renderCategoryTitle }: any) {
  const { message } = App.useApp();
  // ========== 状态定义 ==========

  // 分类树数据：后端返回的扁平列表
  // 结构：[{ id, name, parent_id, sort_order, paper_count, created_at, updated_at }, ...]
  const [categories, setCategories] = useState<any[]>([]); // 所有分类的扁平列表

  // 论文统计数据：后端返回的总数和未分类数
  const [totalCount, setTotalCount] = useState(0); // 论文总数
  const [uncategorizedCount, setUncategorizedCount] = useState(0); // 未分类论文数

  // 加载状态：标记分类数据是否正在从后端获取
  const [loading, setLoading] = useState(true); // 初始为 true（首次加载时显示 loading）

  // 展开的树节点 key 列表：控制哪些分类节点处于展开状态
  // 默认展开所有节点，方便用户看到完整的分类结构
  const [expandedKeys, setExpandedKeys] = useState<string[]>([]); // 展开的节点 key 数组

  // ========== 数据加载 ==========

  /**
   * 加载分类树数据
   *
   * 从后端获取所有分类的扁平列表，并自动展开所有节点。
   * 使用 useCallback 缓存函数引用，避免作为 useEffect 依赖时无限循环。
   */
  const loadCategories = useCallback(async () => {
    try { // 开始 try-catch 错误处理
      // 调用 API 获取分类树数据
      const data = await getCategoryTree(); // 异步调用获取分类树接口

      // 更新分类列表状态，使用 || [] 防御后端返回 null
      setCategories(data.categories || []); // 设置分类列表

      // 更新论文统计数据
      setTotalCount(data.total_count || 0); // 设置论文总数
      setUncategorizedCount(data.uncategorized_count || 0); // 设置未分类论文数

      // 自动展开所有分类节点
      // 将所有分类 ID 转换为字符串作为 expandedKeys
      const allKeys = (data.categories || []).map(c => String(c.id)); // 提取所有分类 ID
      setExpandedKeys(allKeys); // 设置展开的节点
    } catch (err) { // 捕获网络或接口错误
      // 显示错误提示
      message.error('加载分类失败: ' + (err as any).message); // 弹出错误提示
    } finally { // 无论成功失败都执行
      // 关闭加载状态
      setLoading(false); // 停止 loading 动画
    }
  }, []); // 空依赖数组，函数引用稳定

  // ========== 组件挂载时加载 ==========

  /**
   * 组件挂载时加载分类数据
   *
   * 使用 useEffect 确保组件首次渲染时自动加载数据。
   */
  useEffect(() => {
    loadCategories(); // 调用加载分类函数
  }, [loadCategories]); // 依赖 loadCategories 函数

  // ========== 构建树形数据 ==========

  /**
   * 构建完整的树形数据
   *
   * 包含三个部分：
   * 1. "全部论文"虚拟节点：显示所有论文的总数
   * 2. "未分类"虚拟节点：显示未归入任何分类的论文数
   * 3. 用户自定义的分类树：从后端数据递归构建
   *
   * 注意：虚拟节点不存储在后端数据库中，由前端动态生成。
   */
  const treeData = useMemo(() => [
    // 虚拟分类："全部论文"
    {
      key: 'all',
      title: renderCategoryTitle('全部论文', totalCount),
      isLeaf: true,
      selectable: true,
    },
    // 虚拟分类："未分类"
    {
      key: 'uncategorized',
      title: renderCategoryTitle('未分类', uncategorizedCount),
      isLeaf: true,
      selectable: true,
    },
    ...buildTreeData(categories, renderCategoryTitle),
  ], [categories, totalCount, uncategorizedCount, renderCategoryTitle]);

  // ========== 返回值 ==========

  // 返回状态和方法，供组件使用
  return {
    categories, // 所有分类的扁平列表
    totalCount, // 论文总数
    uncategorizedCount, // 未分类论文数
    loading, // 加载状态
    expandedKeys, // 展开的节点 key 数组
    setExpandedKeys, // 设置展开节点的方法
    loadCategories, // 手动加载分类数据的函数
    treeData, // 完整的树形数据（包含虚拟节点）
  };
}

/**
 * 创建分类树 Hook 的 ref 处理器
 *
 * 用于向父组件暴露 refreshCategories 方法，
 * 让父组件可以在删除论文后刷新分类计数。
 *
 * @param {object} ref - 转发的 ref 对象
 * @param {Function} loadCategories - 加载分类数据的函数
 * @returns {void}
 */
export function useCategoryTreeRef(ref: any, loadCategories: any) {
  /**
   * 暴露 loadCategories 给父组件
   *
   * 父组件可以通过 ref.current.refreshCategories() 调用此方法，
   * 用于在删除论文或修改分类后刷新分类计数。
   */
  useImperativeHandle(ref, () => ({
    refreshCategories: loadCategories, // 暴露刷新方法
  }), [loadCategories]); // 依赖 loadCategories 函数
}

/**
 * 默认导出：useCategoryTree Hook
 *
 * 使用示例：
 * ```jsx
 * const { categories, loading, expandedKeys, loadCategories, treeData } = useCategoryTree({
 *   renderCategoryTitle: (name, count) => <span>{name} ({count})</span>,
 * });
 * ```
 */
export default useCategoryTree;
