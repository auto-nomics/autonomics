/**
 * useColumnConfig - 论文表格列配置钩子
 *
 * 本文件封装了论文列表表格的列配置功能，包括：
 * - 列可见性管理（用户自定义显示哪些列）
 * - 列宽度管理（拖拽调整列宽，持久化到 localStorage）
 * - 表头右键菜单（列显示/隐藏切换）
 * - 列排序管理（点击表头排序）
 *
 * 持久化策略：
 * - visibleColumns: 保存到 localStorage.paperListVisibleColumns
 * - columnWidths: 保存到 localStorage.paperListColumnWidths
 * - 用户刷新页面后自动恢复上次的列配置
 *
 * 为什么需要独立 hook？
 * - 列配置逻辑独立，包含多个状态和操作
 * - 需要在多个组件中复用（如导出功能、打印预览）
 * - 集中管理 localStorage 的读写，避免代码分散
 */

import { useState, useCallback, useEffect, useMemo } from 'react'; // 导入 React 核心钩子
import { Checkbox, Space } from 'antd'; // 导入 Ant Design 组件
import { // 从常量文件导入列定义
  ALL_COLUMNS, // 所有可用列定义
  DEFAULT_VISIBLE_COLUMNS, // 默认可见列配置
  SORT_FIELD_MAP, // 列 key 到后端字段名的映射
} from '../paperListConstants';
import usePaperStore from '../../../stores/usePaperStore'; // 导入论文列表 Store

/**
 * 论文表格列配置钩子
 *
 * @param {Function} loadPapers - 刷新论文列表的回调函数（排序变更后调用）
 * @returns {Object} 返回状态和操作函数的对象
 */
export function useColumnConfig(loadPapers: () => void) {
  // ========================================
  // 列可见性管理
  // ========================================

  /**
   * 可见列配置：用户自定义要显示的表格列
   *
   * 值为 ALL_COLUMNS 中的 key 数组，如 ['title', 'authors', 'journal', 'year', ...]
   * 标题列（title）始终包含在内，不受此状态控制。
   *
   * 从 localStorage 恢复用户的列配置，如果没有保存记录则使用默认配置。
   *
   * @type {Array<string>}
   */
  const [visibleColumns, setVisibleColumns] = useState(() => {
    // 从 localStorage 读取上次保存的列配置
    const saved = localStorage.getItem('paperListVisibleColumns'); // 读取 localStorage 中的列配置 JSON

    if (saved) { // 如果有保存记录
      try { // 尝试解析 JSON
        const parsed = JSON.parse(saved); // 解析 JSON 字符串为数组
        // 确保解析结果是数组且包含标题列（防御性检查）
        if (Array.isArray(parsed) && parsed.includes('title')) { // 检查是否为数组且包含 'title'
          return parsed; // 返回解析后的数组
        }
      } catch { /* JSON 解析失败（数据损坏），使用默认值 */ }
    }

    // 没有保存记录或解析失败时，使用默认列配置
    return DEFAULT_VISIBLE_COLUMNS; // 使用默认可见列
  });

  /**
   * 切换列可见性
   *
   * @param {Array<string>} newColumns - 新的可见列 key 数组
   */
  const toggleColumnVisibility = useCallback((newColumns: string[]) => {
    // 确保标题列始终包含在内
    const columnsWithTitle = newColumns.includes('title') // 检查是否包含标题列
      ? newColumns // 已包含，直接使用
      : ['title', ...newColumns]; // 未包含，强制添加到开头

    setVisibleColumns(columnsWithTitle); // 更新状态
    localStorage.setItem('paperListVisibleColumns', JSON.stringify(columnsWithTitle)); // 持久化到 localStorage
  }, []); // 无外部依赖

  // ========================================
  // 列宽度管理
  // ========================================

  /**
   * 列宽配置：存储用户拖拽调整后的列宽
   *
   * 数据格式：{ columnKey: width }，如 { title: 350, authors: 180, ... }
   * 空对象表示使用 ALL_COLUMNS 中定义的默认宽度。
   *
   * 从 localStorage 恢复用户的列宽配置。
   *
   * @type {Object}
   */
  const [columnWidths, setColumnWidths] = useState(() => {
    const saved = localStorage.getItem('paperListColumnWidths'); // 读取 localStorage 中的列宽配置 JSON

    if (saved) { // 如果有保存记录
      try { // 尝试解析 JSON
        return JSON.parse(saved); // 返回解析后的列宽对象
      } catch { /* JSON 解析失败，使用默认值 */ }
    }

    return {}; // 空对象表示使用 ALL_COLUMNS 中的默认宽度
  });

  /**
   * 拖拽调整列宽的处理函数
   *
   * 当用户拖拽表头分隔线调整列宽时调用。
   * 更新状态并持久化到 localStorage。
   *
   * @param {string} key - 列的 key（如 'title', 'authors'）
   * @param {number} width - 新的列宽（像素）
   */
  const handleResize = useCallback((key: string, width: number) => {
    setColumnWidths((prev: any) => { // 更新列宽状态
      const next = { ...prev, [key]: width }; // 浅拷贝并更新指定列的宽度
      localStorage.setItem('paperListColumnWidths', JSON.stringify(next)); // 持久化到 localStorage
      return next; // 返回新的列宽对象
    });
  }, []); // 无外部依赖

  // ========================================
  // 排序管理
  // ========================================

  /**
   * 从 Zustand Store 读取排序状态
   *
   * sortField 和 sortOrder 现在由 usePaperStore 管理，
   * 支持跨组件共享和持久化到 localStorage。
   */
  const sortField = usePaperStore(s => s.sortField); // 当前排序字段（从 Store 读取）
  const sortOrder = usePaperStore(s => s.sortOrder); // 当前排序方向（从 Store 读取）
  const setSortField = usePaperStore(s => s.setSortField); // 设置排序字段（Store 方法）
  const setSortOrder = usePaperStore(s => s.setSortOrder); // 设置排序方向（Store 方法）

  /**
   * 处理排序变更
   *
   * 当用户点击可排序列的表头时调用。
   * 实现两态排序逻辑（升序 ↔ 降序），跳过"取消排序"状态。
   *
   * @param {string} columnKey - 被点击的列 key（如 'year', 'if'）
   */
  const handleSort = useCallback((columnKey: string) => {
    // 将列 key 映射为后端排序字段名
    // 例如：'year' → 'publicationYear'，'if' → 'impactFactor'
    const apiField = (SORT_FIELD_MAP as any)[columnKey] || columnKey; // 如果映射表中没有，直接使用列 key

    if (sortField === apiField) { // 当前已按本列排序
      // 在升序和降序之间切换
      // prev === 'asc' → 'desc'，prev === 'desc' → 'asc'
      setSortOrder(prev => prev === 'asc' ? 'desc' : 'asc');
    } else { // 当前按其他列排序
      // 切换到本列并默认升序
      setSortField(apiField); // 设置新的排序字段（使用映射后的后端字段名）
      setSortOrder('asc'); // 默认升序，提供可预测的初始排序方向
    }

    // 排序变更后触发论文列表重新加载
    // 注意：实际加载由 useEffect 监听 sortField/sortOrder 变化后调用
  }, [sortField, setSortField, setSortOrder]); // 依赖当前排序字段和 Store 方法

  // ========================================
  // 表头右键菜单管理
  // ========================================

  /**
   * 表头右键菜单位置
   *
   * 记录表头右键点击的位置，用于渲染列可见性切换菜单。
   * null 表示隐藏菜单，{ x, y } 表示显示位置。
   *
   * @type {{ x: number, y: number } | null}
   */
  const [headerContextMenu, setHeaderContextMenu] = useState<{ x: number; y: number } | null>(null); // null 表示隐藏，{ x, y } 表示显示位置

  /**
   * 关闭表头右键菜单
   */
  const closeHeaderContextMenu = useCallback(() => {
    setHeaderContextMenu(null); // 隐藏菜单
  }, []); // 无外部依赖

  /**
   * 处理表头右键点击事件
   *
   * @param {Object} e - 右键事件对象
   */
  const handleHeaderContextMenu = useCallback((e: any) => {
    e.preventDefault(); // 阻止浏览器默认右键菜单
    setHeaderContextMenu({ x: e.clientX, y: e.clientY }); // 记录鼠标位置
  }, []); // 无外部依赖

  /**
   * 点击页面其他区域时关闭表头右键菜单
   */
  useEffect(() => {
    // 如果菜单未显示，跳过
    if (!headerContextMenu) return; // 无菜单时跳过

    // 点击事件监听器
    const close = () => setHeaderContextMenu(null); // 关闭菜单
    document.addEventListener('click', close); // 注册点击事件

    // 清理函数：移除事件监听器
    return () => document.removeEventListener('click', close); // 组件卸载时清理
  }, [headerContextMenu]); // 依赖菜单状态

  // ========================================
  // 活跃列配置计算
  // ========================================

  /**
   * 计算表格总宽度
   *
   * 所有可见列宽度之和，用于 Table 的 scroll.x 属性。
   * 使用精确的列宽之和而非 'max-content'，配合 table-layout:fixed，
   * 确保列严格遵守指定宽度，不会被内容撑开或按比例扩展。
   *
   * @returns {number} 表格总宽度（像素）
   */
  const tableScrollX = useMemo(() => {
    return visibleColumns
      .map(key => { // 遍历可见列 key
        const col = ALL_COLUMNS.find(c => c.key === key); // 查找列定义
        const defaultWidth = col?.width || 150; // 默认宽度 150px
        const customWidth = columnWidths[key]; // 用户自定义宽度
        return customWidth || defaultWidth; // 优先使用自定义宽度
      })
      .reduce((sum, width) => sum + width, 0); // 求和
  }, [visibleColumns, columnWidths]); // 依赖可见列和列宽配置

  // ========================================
  // 导出公共接口
  // ========================================

  return {
    // 列可见性
    visibleColumns, // 当前可见的列 key 数组
    toggleColumnVisibility, // 切换列可见性函数

    // 列宽度
    columnWidths, // 列宽配置对象 { columnKey: width }
    handleResize, // 拖拽调整列宽函数

    // 排序
    sortField, // 当前排序字段（后端字段名）
    sortOrder, // 当前排序方向 'asc' | 'desc'
    setSortField, // 设置排序字段（外部直接控制时使用）
    setSortOrder, // 设置排序方向（外部直接控制时使用）
    handleSort, // 处理排序变更函数

    // 表头右键菜单
    headerContextMenu, // 表头右键菜单位置 { x, y } | null
    setHeaderContextMenu, // 设置表头右键菜单位置（直接控制时使用）
    closeHeaderContextMenu, // 关闭表头右键菜单函数
    handleHeaderContextMenu, // 处理表头右键点击函数

    // 计算属性
    tableScrollX, // 表格总宽度（用于 scroll.x）

    // 常量（供外部使用）
    ALL_COLUMNS, // 所有列定义
    SORT_FIELD_MAP, // 排序字段映射表
  };
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} ColumnWidths
 * @property {number} [title] - 标题列宽度
 * @property {number} [authors] - 作者列宽度
 * @property {number} [journal] - 期刊列宽度
 * @property {number} [year] - 年份列宽度
 * @property {number} [citations] - 被引次数列宽度
 * @property {number} [if] - 影响因子列宽度
 * @property {number} [partition] - 分区列宽度
 * @property {number} [added] - 添加时间列宽度
 * @property {string} [key: string] - 其他列的宽度
 */
