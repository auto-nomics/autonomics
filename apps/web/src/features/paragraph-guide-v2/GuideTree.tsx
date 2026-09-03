/**
 * JayRead V2 层次化导览树容器组件
 *
 * 负责管理和渲染完整的导览树结构，包括：
 * 1. 树形数据管理（展开/折叠状态）
 * 2. 节点导航回调处理
 * 3. 虚拟滚动支持（可选）
 * 4. 搜索和过滤功能预留
 * 5. 反向导航：滚动到指定节点（PDF 点击 → 导览定位）
 *
 * @module GuideTree
 */

import React, { useState, useCallback, useMemo, useRef, useImperativeHandle, forwardRef } from 'react'; // 导入 React 核心钩子
import PropTypes from 'prop-types'; // 导入 PropTypes 类型检查库
import { Empty, Spin } from 'antd'; // 导入 Ant Design 组件：Empty 空状态、Spin 加载动画
import GuideTreeNode from './GuideTreeNode'; // 导入树节点组件

/**
 * 层次化导览树容器组件（使用 forwardRef 包装，支持父组件命令式调用）
 *
 * @param {object} props - 组件属性
 * @param {Array<object>} props.nodes - 导览节点数组（扁平化或树形结构）
 * @param {boolean} props.loading - 是否正在加载
 * @param {Function} props.onNodeClick - 节点点击回调：(node) => void
 * @param {string} [props.expandLevel] - 默认展开层级（'all' | 'first' | 'none'）
 * @param {React.Ref} ref - 转发的 ref，用于暴露命令式 API（scrollToNode 方法）
 */
const GuideTree = forwardRef<any, any>(function GuideTree({
  nodes,
  loading,
  onNodeClick,
  expandLevel = 'first', // 默认展开第一层
}, ref) {
  // ========== 状态管理 ==========

  /**
   * 展开的节点 ID 集合
   *
   * 使用 Set 存储已展开节点的 ID，便于快速查找和更新。
   */
  const [expandedIds, setExpandedIds] = useState(new Set()); // 状态：展开的节点 ID 集合

  /**
   * 节点 DOM 元素引用映射表
   *
   * 用于反向导航时快速定位并滚动到指定节点。
   * key 为节点 ID，value 为对应的 DOM 元素。
   */
  const nodeElementRefs = useRef(new Map()); // ref：存储节点 DOM 元素的映射表

  // ========== 计算属性 ==========

  /**
   * 构建树形结构数据
   *
   * 如果传入的 nodes 是扁平化结构（每个节点有 parentId），
   * 则将其转换为树形结构（每个节点的 children 属性包含子节点）。
   *
   * 如果已经是树形结构，则直接使用。
   */
  const treeData = useMemo(() => {
    // 检查是否已经是树形结构（第一个节点有 children 属性）
    if (nodes.length > 0 && nodes[0].children) {
      return nodes; // 已经是树形结构，直接返回
    }

    // 扁平化结构转树形结构
    /**
     * 第一步：构建节点映射表
     *
     * 创建一个 Map，key 为节点 ID，value 为节点对象。
     * 方便快速查找节点和添加子节点。
     */
    const nodeMap = new Map(); // 节点映射表

    // 初始化映射表，为每个节点添加空的 children 数组
    nodes.forEach((node: any) => {
      nodeMap.set(node.id, { ...node, children: [] }); // 复制节点对象，避免修改原数据
    });

    /**
     * 第二步：构建父子关系
     *
     * 遍历所有节点，根据 parentId 将子节点添加到父节点的 children 数组中。
     * 没有 parentId 的节点作为根节点。
     */
    const rootNodes: any[] = []; // 根节点数组

    nodes.forEach((node: any) => {
      const enrichedNode = nodeMap.get(node.id); // 获取 enriched 节点（包含 children）

      if (node.parentId) {
        // 有父节点：添加到父节点的 children
        const parent = nodeMap.get(node.parentId);
        if (parent) {
          parent.children.push(enrichedNode); // 添加到父节点的 children
        } else {
          // 父节点不存在，作为根节点处理
          rootNodes.push(enrichedNode);
        }
      } else {
        // 无父节点：作为根节点
        rootNodes.push(enrichedNode);
      }
    });

    return rootNodes; // 返回根节点数组
  }, [nodes]); // 依赖于 nodes

  // ========== 初始化默认展开状态 ==========

  /**
   * Effect: 根据 expandLevel 初始化展开状态
   *
   * 当 treeData 或 expandLevel 变化时，自动展开对应层级的节点。
   */
  React.useEffect(() => {
    /**
     * 收集需要展开的节点 ID
     *
     * @param {Array<object>} nodeList - 节点列表
     * @param {number} currentDepth - 当前深度
     * @param {Set<string>} ids - 收集的 ID 集合
     */
    const collectExpandedIds = (nodeList: any[], currentDepth: number, ids: Set<any>) => {
      nodeList.forEach((node: any) => {
        // 根据展开层级决定是否展开
        if (expandLevel === 'all' || (expandLevel === 'first' && currentDepth < 1)) {
          ids.add(node.id); // 添加到展开集合
        }

        // 递归处理子节点
        if (node.children && node.children.length > 0) {
          collectExpandedIds(node.children, currentDepth + 1, ids);
        }
      });
    };

    // 创建新的展开集合
    const newExpandedIds = new Set();
    collectExpandedIds(treeData, 0, newExpandedIds);

    // 更新状态
    setExpandedIds(newExpandedIds);
  }, [treeData, expandLevel]); // 依赖于 treeData 和 expandLevel

  // ========== 事件处理函数 ==========

  /**
   * 切换节点展开/折叠状态
   *
   * @param {string} nodeId - 节点 ID
   */
  const handleToggle = useCallback((nodeId: any) => {
    setExpandedIds((prev) => {
      const newSet = new Set(prev); // 克隆旧集合

      // 切换展开状态
      if (newSet.has(nodeId)) {
        newSet.delete(nodeId); // 已展开 → 折叠
      } else {
        newSet.add(nodeId); // 已折叠 → 展开
      }

      return newSet; // 返回新集合
    });
  }, []); // 无依赖项

  /**
   * 展开所有节点
   */
  const handleExpandAll = useCallback(() => {
    /**
     * 收集所有节点 ID
     *
     * @param {Array<object>} nodeList - 节点列表
     * @param {Set<string>} ids - 收集的 ID 集合
     */
    const collectAllIds = (nodeList: any[], ids: Set<any>) => {
      nodeList.forEach((node: any) => {
        ids.add(node.id); // 添加到集合

        // 递归处理子节点
        if (node.children && node.children.length > 0) {
          collectAllIds(node.children, ids);
        }
      });
    };

    const allIds = new Set();
    collectAllIds(treeData, allIds);

    setExpandedIds(allIds); // 展开所有节点
  }, [treeData]); // 依赖于 treeData

  /**
   * 折叠所有节点
   */
  const handleCollapseAll = useCallback(() => {
    setExpandedIds(new Set()); // 清空展开集合
  }, []); // 无依赖项

  /**
   * 注册节点 DOM 元素
   *
   * 由 GuideTreeNode 组件调用，将节点的 DOM 元素注册到映射表中。
   * 用于反向导航时快速定位节点。
   *
   * @param {string} nodeId - 节点 ID
   * @param {HTMLElement} element - 节点的 DOM 元素
   */
  const registerNodeElement = useCallback((nodeId: any, element: any) => {
    if (element) {
      nodeElementRefs.current.set(nodeId, element);
    } else {
      nodeElementRefs.current.delete(nodeId);
    }
  }, []); // 无依赖项

  /**
   * 构建节点 ID 到路径的映射
   *
   * 用于反向导航时，找到从根节点到目标节点的完整路径，
   * 以便展开所有父节点。
   *
   * @param {Array<object>} nodeList - 节点列表
   * @param {string} targetId - 目标节点 ID
   * @param {Array<string>} path - 当前路径（累积）
   * @returns {Array<string>|null} 从根到目标节点的 ID 路径，找不到时返回 null
   */
  const findNodePath = useCallback((nodeList: any[], targetId: any, path: any[] = []): any[] | null => {
    for (const node of nodeList) {
      const currentPath: any[] = [...path, node.id];

      // 找到目标节点
      if (node.id === targetId) {
        return currentPath;
      }

      // 递归搜索子节点
      if (node.children && node.children.length > 0) {
        const result: any[] | null = findNodePath(node.children, targetId, currentPath);
        if (result) {
          return result;
        }
      }
    }
    return null; // 未找到
  }, []); // 无依赖项

  /**
   * 命令式 API（暴露给父组件）
   *
   * 提供 scrollToNode 方法，用于反向导航（PDF 点击 → 导览滚动）。
   */
  useImperativeHandle(ref, () => ({
    /**
     * 滚动到指定节点并高亮显示
     *
     * 实现步骤：
     * 1. 找到从根节点到目标节点的路径
     * 2. 展开路径上的所有父节点
     * 3. 滚动到目标节点的 DOM 元素
     * 4. 添加临时高亮样式
     *
     * @param {string} nodeId - 目标节点 ID
     */
    scrollToNode: (nodeId: any) => {
      // 第一步：找到节点路径
      const path = findNodePath(treeData, nodeId);
      if (!path) {
        console.warn('[GuideTree] 未找到节点:', nodeId);
        return;
      }

      // 第二步：展开路径上的所有父节点（排除目标节点本身）
      const parentIds = path.slice(0, -1);
      setExpandedIds((prev) => {
        const newSet = new Set(prev);
        parentIds.forEach(id => newSet.add(id));
        return newSet;
      });

      // 第三步：等待 DOM 更新后滚动到目标节点
      // 使用 setTimeout 确保 React 完成重新渲染
      setTimeout(() => {
        const element = nodeElementRefs.current.get(nodeId);
        if (element) {
          // 滚动到可见区域，使用 block: 'center' 让节点居中显示
          element.scrollIntoView({
            behavior: 'smooth',
            block: 'center',
          });

          // 添加临时高亮效果
          element.style.transition = 'background-color 0.3s';
          element.style.backgroundColor = 'color-mix(in srgb, var(--color-primary) 15%, transparent)';

          // 2 秒后移除高亮
          setTimeout(() => {
            element.style.backgroundColor = '';
          }, 2000);
        } else {
          console.warn('[GuideTree] 未找到节点 DOM 元素:', nodeId);
        }
      }, 100);
    },
  }), [treeData, findNodePath]); // 依赖于 treeData 和 findNodePath

  // ========== 渲染 ==========

  /**
   * 加载状态
   *
   * 如果正在加载，显示加载动画。
   */
  if (loading) {
    return (
      <div
        style={{
          display: 'flex',
          justifyContent: 'center',
          alignItems: 'center',
          padding: 24,
        }}
      >
        <Spin tip="加载导览中..."><div /></Spin>
      </div>
    );
  }

  /**
   * 空状态
   *
   * 如果没有节点数据，显示空状态提示。
   */
  if (!treeData || treeData.length === 0) {
    return (
      <div style={{ padding: 24 }}>
        <Empty description="暂无导览数据" /> // 空状态组件
      </div>
    );
  }

  /**
   * 主渲染：树形结构
   *
   * 遍历根节点数组，递归渲染每个节点。
   */
  return (
    <div className="guide-tree-container">
      {/**
       * 工具栏（可选）
       *
       * 提供展开全部/折叠全部按钮。
       */}
      <div
        style={{
          display: 'flex',
          justifyContent: 'flex-end',
          padding: '4px 8px',
          borderBottom: '1px solid var(--border-color)',
          marginBottom: 8,
        }}
      >
        {/* 展开/折叠工具栏预留位置，可根据需要添加按钮 */}
      </div>

      {/**
       * 树节点列表
       *
       * 遍历根节点，渲染每个 GuideTreeNode。
       */}
      <div className="guide-tree-nodes">
        {treeData.map((node: any) => (
          <GuideTreeNode
            key={node.id} // React key，使用节点 ID
            node={node} // 节点数据
            depth={0} // 根节点深度为 0
            onNodeClick={onNodeClick} // 导航回调
            onToggle={handleToggle} // 展开/折叠回调
            isExpanded={expandedIds.has(node.id)} // 是否展开
            onRegisterElement={registerNodeElement} // 注册 DOM 元素（用于反向导航）
          />
        ))}
      </div>
    </div>
  );
});

// ========== PropTypes 类型检查 ==========

/**
 * 组件属性类型检查
 */
GuideTree.propTypes = {
  /**
   * 导览节点数组
   *
   * 可以是扁平化结构（每个节点有 parentId）或树形结构（每个节点有 children）。
   */
  nodes: PropTypes.arrayOf(
    PropTypes.shape({
      id: PropTypes.string.isRequired, // 节点唯一标识（必需）
      title: PropTypes.string.isRequired, // 节点标题（必需）
      parentId: PropTypes.string, // 父节点 ID（可选，扁平化结构时使用）
      children: PropTypes.array, // 子节点数组（可选，树形结构时使用）
    })
  ).isRequired,

  /**
   * 是否正在加载
   */
  loading: PropTypes.bool, // 加载状态（可选）

  /**
   * 节点点击回调
   *
   * 点击节点时触发，传递节点数据。
   */
  onNodeClick: PropTypes.func, // 点击回调（可选）

  /**
   * 默认展开层级
   *
   * - 'all': 展开所有层级
   * - 'first': 只展开第一层
   * - 'none': 全部折叠
   */
  expandLevel: PropTypes.oneOf(['all', 'first', 'none']), // 展开层级（可选）
};

/**
 * 默认导出：GuideTree 组件
 */
export default GuideTree;
