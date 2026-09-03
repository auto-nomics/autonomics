/**
 * 分栏系统类型定义模块
 *
 * 定义分栏树数据结构的所有 TypeScript 类型，包括：
 * - 基础类型别名：PaneId、Direction、SplitPosition、PrefixKeyState
 * - 节点接口：SplitNode（分割节点）、LeafNode（叶子节点）
 * - 联合类型：PaneNode（节点联合类型）
 * - 操作接口：PendingSplit（待执行分割）、PendingNavigation（待执行导航）
 *
 * 分栏树数据结构说明：
 * - 分栏树是一棵二叉树，每个内部节点（SplitNode）有两个子节点
 * - 叶子节点（LeafNode）是树的终端，承载实际的路由内容
 * - 树的结构决定了面板的布局：方向、比例、嵌套关系
 * - 所有节点通过唯一 ID 标识，支持高效的查找和替换操作
 *
 * @module panes/types
 */

/**
 * 面板唯一标识符类型
 * 使用字符串类型，由 generateId() 生成
 */
export type PaneId = string;

/**
 * 分割方向类型
 * - 'horizontal': 水平分割（左右排列）
 * - 'vertical': 垂直分割（上下排列）
 */
export type Direction = 'horizontal' | 'vertical';

/**
 * 分割位置类型
 * - 'before': 新面板放在目标面板前面（左侧/上方）
 * - 'after': 新面板放在目标面板后面（右侧/下方）
 */
export type SplitPosition = 'before' | 'after';

/**
 * 前缀键状态类型
 *
 * 描述 Ctrl+B 前缀键操作的当前状态：
 * - 'idle': 空闲状态，未按前缀键
 * - 'pending': 已按 Ctrl+B，等待后续操作键（h/l/j/k 等）
 * - 'pendingSplit': 分屏模式，等待按页面选择键（a/p/n/b/w/s）
 */
export type PrefixKeyState = 'idle' | 'active' | 'pendingSplit';

/**
 * 待执行的分割操作配置接口
 *
 * 存储用户通过前缀键发起的分割操作参数，
 * 在用户按下页面选择键后执行实际的分割操作。
 */
export interface PendingSplit {
  /** 分割方向（水平/垂直） */
  direction: Direction;
  /** 新面板的位置（前/后） */
  position: SplitPosition;
}

/**
 * 分割节点接口
 *
 * 分栏树的内部节点，包含两个子节点和分割配置。
 * 通过 direction 控制子节点的排列方向，splitRatio 控制子节点的尺寸比例。
 */
export interface SplitNode {
  /** 节点唯一标识符 */
  id: PaneId;
  /** 节点类型标识：'split' 表示分割节点 */
  type: 'split';
  /** 分割方向：'horizontal'（左右）或 'vertical'（上下） */
  direction: Direction;
  /** 分割比例（0-1），控制第一个子节点的尺寸占比 */
  splitRatio: number;
  /** 第一个子节点（左/上） */
  first: PaneNode;
  /** 第二个子节点（右/下） */
  second: PaneNode;
}

/**
 * 叶子节点接口
 *
 * 分栏树的终端节点，承载实际的路由内容。
 * 每个叶子节点对应一个独立的 MemoryRouter 实例，
 * 支持在同一页面中同时运行多个路由视图。
 */
export interface LeafNode {
  /** 节点唯一标识符 */
  id: PaneId;
  /** 节点类型标识：'leaf' 表示叶子节点 */
  type: 'leaf';
  /** 当前路由路径（如 '/papers'、'/paper/123'） */
  route: string;
  /** URL 查询参数（如 '?tab=info'） */
  search: string;
}

/**
 * 节点联合类型
 *
 * 分栏树中的节点可以是分割节点或叶子节点。
 * 使用联合类型实现类型安全的节点操作。
 */
export type PaneNode = SplitNode | LeafNode;

/**
 * 待执行的导航操作接口
 *
 * 存储来自全局快捷键的导航请求，
 * 由 PaneLeaf 中的 RouteSync 组件消费并执行。
 */
export interface PendingNavigation {
  /** 目标面板 ID */
  paneId: PaneId;
  /** 导航目标路径（如 '/papers'、'/paper/123'） */
  path: string;
}
