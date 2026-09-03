/**
 * 分栏树工具函数模块
 *
 * 提供分栏树数据结构的不可变操作工具函数，包括：
 * - 节点生成：generateId、createLeaf
 * - 节点查找：findNodeById、findParentOf
 * - 节点遍历：getAllLeafIds、leafCount
 * - 节点替换：replaceNode
 * - 叶子节点操作：removeLeaf、splitLeaf
 * - 相邻节点查找：findAdjacentLeaf
 * - 状态判断：hasMultiplePanes
 *
 * 设计原则：
 * - 所有函数都是纯函数，不修改原始数据结构
 * - 返回新的节点树（不可变更新模式）
 * - 使用递归遍历树结构
 *
 * @module panes/treeUtils
 */
import { PaneId, PaneNode, SplitNode, LeafNode, Direction, SplitPosition } from './types';

/**
 * 生成唯一的面板 ID
 *
 * 使用当前时间戳的 36 进制表示 + 随机字符串，确保 ID 的唯一性。
 * 格式示例："lxyz123abc"
 *
 * @returns {PaneId} 唯一的面板 ID 字符串
 */
export function generateId(): PaneId {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

/**
 * 创建新的叶子节点
 *
 * 生成一个新的 LeafNode，包含唯一 ID 和路由信息。
 *
 * @param {string} [route='/'] - 初始路由路径（默认为首页）
 * @param {string} [search=''] - URL 查询参数（默认为空）
 * @returns {LeafNode} 新创建的叶子节点
 */
export function createLeaf(route: string = '/', search: string = ''): LeafNode {
  return { id: generateId(), type: 'leaf', route, search };
}

/**
 * 根据 ID 查找节点
 *
 * 递归遍历分栏树，查找指定 ID 的节点。
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} id - 要查找的节点 ID
 * @returns {PaneNode | null} 找到的节点，未找到返回 null
 */
export function findNodeById(root: PaneNode, id: PaneId): PaneNode | null {
  if (root.id === id) return root; // 找到目标节点
  if (root.type === 'split') {
    // 递归查找左右子树
    return findNodeById(root.first, id) || findNodeById(root.second, id);
  }
  return null; // 叶子节点且不是目标，返回 null
}

/**
 * 查找目标节点的父节点
 *
 * 递归遍历分栏树，查找包含目标节点 ID 的父分割节点。
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} targetId - 目标节点 ID
 * @returns {{ parent: SplitNode; side: 'first' | 'second' } | null}
 *   找到时返回父节点和目标所在侧（first/second），未找到返回 null
 */
export function findParentOf(
  root: PaneNode,
  targetId: PaneId
): { parent: SplitNode; side: 'first' | 'second' } | null {
  if (root.type === 'leaf') return null; // 叶子节点没有子节点
  // 检查左子节点是否为目标
  if (root.first.id === targetId) return { parent: root, side: 'first' };
  // 检查右子节点是否为目标
  if (root.second.id === targetId) return { parent: root, side: 'second' };
  // 递归查找左右子树
  return findParentOf(root.first, targetId) || findParentOf(root.second, targetId);
}

/**
 * 获取所有叶子节点的 ID 列表
 *
 * 递归遍历分栏树，收集所有叶子节点的 ID。
 * 返回的 ID 列表按照树的深度优先遍历顺序排列。
 *
 * @param {PaneNode} root - 树的根节点
 * @returns {PaneId[]} 所有叶子节点的 ID 数组
 */
export function getAllLeafIds(root: PaneNode): PaneId[] {
  if (root.type === 'leaf') return [root.id]; // 叶子节点：返回自身 ID
  // 分割节点：合并左右子树的叶子 ID
  return [...getAllLeafIds(root.first), ...getAllLeafIds(root.second)];
}

/**
 * 获取叶子节点总数
 *
 * @param {PaneNode} root - 树的根节点
 * @returns {number} 叶子节点数量
 */
export function leafCount(root: PaneNode): number {
  return getAllLeafIds(root).length;
}

/**
 * 替换树中的指定节点
 *
 * 递归遍历分栏树，将指定 ID 的节点替换为新的节点。
 * 不修改原始树，返回新的树结构（不可变更新）。
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} targetId - 要替换的节点 ID
 * @param {PaneNode} replacement - 替换后的新节点
 * @returns {PaneNode} 替换后的新树
 */
export function replaceNode(
  root: PaneNode,
  targetId: PaneId,
  replacement: PaneNode
): PaneNode {
  if (root.id === targetId) return replacement; // 找到目标，返回替换节点
  if (root.type === 'split') {
    // 递归替换左右子树，返回新的分割节点
    return {
      ...root,
      first: replaceNode(root.first, targetId, replacement),
      second: replaceNode(root.second, targetId, replacement),
    };
  }
  return root; // 叶子节点且不是目标，返回原节点
}

/**
 * 移除指定的叶子节点
 *
 * 递归遍历分栏树，移除指定 ID 的叶子节点。
 * 移除规则：
 * - 如果目标叶子是分割节点的直接子节点，返回另一个子节点（自动"上提"）
 * - 如果目标叶子在更深层级，递归移除后返回更新后的树
 * - 如果只剩一个叶子节点（根节点），返回 null
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} leafId - 要移除的叶子节点 ID
 * @returns {PaneNode | null} 移除后的新树，如果无法移除返回 null
 */
export function removeLeaf(
  root: PaneNode,
  leafId: PaneId
): PaneNode | null {
  if (root.type === 'leaf') return null; // 只剩一个叶子，无法移除
  // 如果目标是左子节点，返回右子节点（自动上提）
  if (root.first.id === leafId) return root.second;
  // 如果目标是右子节点，返回左子节点（自动上提）
  if (root.second.id === leafId) return root.first;

  // 递归在左子树中查找并移除
  const firstResult = root.first.type === 'split' ? removeLeaf(root.first, leafId) : null;
  if (firstResult !== null) {
    return { ...root, first: firstResult }; // 左子树已更新
  }
  // 递归在右子树中查找并移除
  const secondResult = root.second.type === 'split' ? removeLeaf(root.second, leafId) : null;
  if (secondResult !== null) {
    return { ...root, second: secondResult }; // 右子树已更新
  }
  return null; // 未找到目标叶子
}

/**
 * 分割指定的叶子节点
 *
 * 将目标叶子节点替换为一个新的分割节点，包含原叶子和新叶子。
 * 分割方向和位置由参数指定。
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} targetLeafId - 要分割的叶子节点 ID
 * @param {Direction} direction - 分割方向（horizontal=水平, vertical=垂直）
 * @param {SplitPosition} position - 新叶子的位置（before=前, after=后）
 * @param {string} [newLeafRoute='/'] - 新叶子的初始路由
 * @param {string} [newLeafSearch=''] - 新叶子的查询参数
 * @returns {PaneNode} 分割后的新树
 */
export function splitLeaf(
  root: PaneNode,
  targetLeafId: PaneId,
  direction: Direction,
  position: SplitPosition,
  newLeafRoute: string = '/',
  newLeafSearch: string = ''
): PaneNode {
  // 查找目标叶子节点
  const target = findNodeById(root, targetLeafId);
  if (!target || target.type !== 'leaf') return root; // 未找到或不是叶子节点

  // 创建新叶子节点
  const newLeaf = createLeaf(newLeafRoute, newLeafSearch);
  // 根据位置创建新的分割节点
  const newSplit: SplitNode = position === 'before'
    ? { id: generateId(), type: 'split', direction, splitRatio: 0.5, first: newLeaf, second: target }
    : { id: generateId(), type: 'split', direction, splitRatio: 0.5, first: target, second: newLeaf };

  // 替换原叶子节点为新的分割节点
  return replaceNode(root, targetLeafId, newSplit);
}

/**
 * 查找相邻的叶子节点
 *
 * 基于树的深度优先遍历顺序，查找指定方向上的相邻叶子节点。
 * 简化实现：使用叶子 ID 列表的线性顺序来确定相邻关系。
 *
 * @param {PaneNode} root - 树的根节点
 * @param {PaneId} fromId - 起始叶子节点 ID
 * @param {'up' | 'down' | 'left' | 'right'} direction - 查找方向
 * @returns {PaneId | null} 相邻叶子节点的 ID，未找到返回 null
 */
export function findAdjacentLeaf(
  root: PaneNode,
  fromId: PaneId,
  direction: 'up' | 'down' | 'left' | 'right'
): PaneId | null {
  // 获取所有叶子节点 ID 列表
  const leaves = getAllLeafIds(root);
  const idx = leaves.indexOf(fromId);
  if (idx === -1) return null; // 未找到起始节点

  // 基于线性顺序的简化空间导航
  switch (direction) {
    case 'left':
    case 'up':
      return idx > 0 ? leaves[idx - 1] : null; // 返回前一个叶子
    case 'right':
    case 'down':
      return idx < leaves.length - 1 ? leaves[idx + 1] : null; // 返回后一个叶子
    default:
      return null;
  }
}

/**
 * 判断是否存在多个面板
 *
 * 如果根节点是分割节点，说明存在多个面板（至少两个叶子节点）。
 *
 * @param {PaneNode} root - 树的根节点
 * @returns {boolean} 是否存在多个面板
 */
export function hasMultiplePanes(root: PaneNode): boolean {
  return root.type === 'split';
}
