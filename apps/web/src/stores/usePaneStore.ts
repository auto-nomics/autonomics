/**
 * 多面板布局状态管理 Store
 *
 * 管理面板树结构和布局状态：
 * - 面板的分割（水平/垂直）
 * - 面板的激活和关闭
 * - 面板的路由管理
 * - 前缀键模式（类似 tmux 的 Ctrl+B 前缀键）
 * - 面板最大化状态
 */
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { PaneId, PaneNode, Direction, SplitPosition, PrefixKeyState, PendingNavigation, PendingSplit } from '../panes/types';
import {
  createLeaf,
  splitLeaf,
  removeLeaf,
  findNodeById,
  leafCount,
  findAdjacentLeaf,
  hasMultiplePanes,
} from '../panes/treeUtils';

// 最大面板数量限制
const MAX_PANES = 8;

interface PaneStore {
  // 面板树的根节点
  root: PaneNode;
  // 当前激活的面板 ID
  activePaneId: PaneId;
  // 最大化的面板 ID（null 表示无面板最大化）
  maximizedPaneId: PaneId | null;
  // 前缀键状态：'idle' | 'active' | 'pendingSplit'
  prefixKeyState: PrefixKeyState;
  // 前缀键超时定时器
  prefixKeyTimer: ReturnType<typeof setTimeout> | null;
  // 待处理的导航操作（用于前缀键后输入路由跳转）
  pendingNavigation: PendingNavigation | null;
  // 待处理的分屏操作（方向和位置）
  pendingSplit: PendingSplit | null;
  // 分屏超时定时器（300ms 内输入导航键则跳转，否则克隆当前页面分屏）
  pendingSplitTimer: ReturnType<typeof setTimeout> | null;

  // 分割指定方向的面板
  splitPane: (direction: Direction, position: SplitPosition) => void;
  // 关闭指定面板
  closePane: (paneId: PaneId) => void;
  // 设置当前激活的面板
  setActivePane: (paneId: PaneId) => void;
  // 设置分屏节点的分割比例（0.15-0.85）
  setSplitRatio: (splitNodeId: PaneId, ratio: number) => void;
  // 更新面板的路由和搜索参数
  updatePaneRoute: (paneId: PaneId, route: string, search: string) => void;
  // 激活前缀键模式（类似 tmux 的 Ctrl+B）
  activatePrefixKey: () => void;
  // 取消前缀键模式
  cancelPrefixKey: () => void;
  // 导航活跃面板到指定路径
  navigateActivePane: (path: string) => void;
  // 消费待处理的导航操作
  consumePendingNavigation: () => PendingNavigation | null;
  // 切换当前面板的最大化状态
  toggleMaximize: () => void;
  // 重置所有面板状态为默认值
  reset: () => void;
  // 检查是否有多个面板
  getHasMultiplePanes: () => boolean;
  // 设置待处理的分屏操作（进入 pendingSplit 状态）
  setPendingSplit: (direction: Direction, position: SplitPosition) => void;
  // 清除待处理的分屏操作
  clearPendingSplit: () => void;
  // 执行待处理的分屏操作（可指定目标路由）
  executePendingSplit: (targetRoute?: string) => void;
}

const initialLeaf = createLeaf('/');

const usePaneStore = create<PaneStore>()(
  persist(
    (set, get) => ({
      root: initialLeaf,
      activePaneId: initialLeaf.id,
      maximizedPaneId: null,
      prefixKeyState: 'idle',
      prefixKeyTimer: null,
      pendingNavigation: null,
      pendingSplit: null,
      pendingSplitTimer: null,

      /** 分割面板 - 在当前激活面板的指定位置创建新面板，克隆当前路由到新面板 */
      splitPane: (direction, position) => {
        const { root, activePaneId } = get();
        if (leafCount(root) >= MAX_PANES) return;

        const activeNode = findNodeById(root, activePaneId);
        const route = activeNode?.type === 'leaf' ? activeNode.route : '/';
        const search = activeNode?.type === 'leaf' ? activeNode.search : '';

        const newRoot = splitLeaf(root, activePaneId, direction, position, route, search);
        if (newRoot === root) return;

        const oldLeaves = new Set(getAllLeafIdsFromNode(root));
        const newLeaves = getAllLeafIdsFromNode(newRoot);
        const newLeafId = newLeaves.find(id => !oldLeaves.has(id));

        set({
          root: newRoot,
          activePaneId: newLeafId || activePaneId,
        });
      },

      /** 关闭面板 - 移除指定面板，如果只剩一个面板则不关闭，自动激活相邻面板 */
      closePane: (paneId) => {
        const { root, activePaneId } = get();
        if (leafCount(root) <= 1) return;

        const newRoot = removeLeaf(root, paneId);
        if (!newRoot) return;

        const remainingLeaves = getAllLeafIdsFromNode(newRoot);
        const newActiveId = remainingLeaves.includes(activePaneId)
          ? activePaneId
          : remainingLeaves[remainingLeaves.length - 1];

        set({ root: newRoot, activePaneId: newActiveId });
      },

      /** 设置激活面板 - 将指定面板设为当前激活状态，用于键盘导航 */
      setActivePane: (paneId) => {
        set({ activePaneId: paneId });
      },

      /** 设置分割比例 - 调整分割节点的宽度比例，限制在 15%-85% 之间 */
      setSplitRatio: (splitNodeId, ratio) => {
        const { root } = get();
        const node = findNodeById(root, splitNodeId);
        if (!node || node.type !== 'split') return;

        const clampedRatio = Math.min(0.85, Math.max(0.15, ratio));
        const newRoot = replaceSplitRatio(root, splitNodeId, clampedRatio);
        set({ root: newRoot });
      },

      /** 更新面板路由 - 修改叶子节点的路由和搜索参数，用于导航到新页面 */
      updatePaneRoute: (paneId, route, search) => {
        const { root } = get();
        const node = findNodeById(root, paneId);
        if (!node || node.type !== 'leaf') return;
        if (node.route === route && node.search === search) return;

        const newRoot = updateLeafRoute(root, paneId, route, search);
        set({ root: newRoot });
      },

      /** 激活前缀键 - 开启 tmux 风格的 Ctrl+B 前缀键模式，2秒后自动取消 */
      activatePrefixKey: () => {
        const { prefixKeyTimer } = get();
        if (prefixKeyTimer) clearTimeout(prefixKeyTimer);

        const timer = setTimeout(() => {
          set({ prefixKeyState: 'idle', prefixKeyTimer: null });
        }, 2000);

        set({ prefixKeyState: 'active', prefixKeyTimer: timer });
      },

      /** 取消前缀键 - 重置前缀键状态，清除所有待处理操作 */
      cancelPrefixKey: () => {
        const { prefixKeyTimer, pendingSplitTimer } = get();
        if (prefixKeyTimer) clearTimeout(prefixKeyTimer);
        if (pendingSplitTimer) clearTimeout(pendingSplitTimer);
        set({ prefixKeyState: 'idle', prefixKeyTimer: null, pendingSplit: null, pendingSplitTimer: null });
      },

      /** 导航活跃面板 - 设置待处理的导航操作，等待确认后执行 */
      navigateActivePane: (path) => {
        const { activePaneId } = get();
        set({ pendingNavigation: { paneId: activePaneId, path } });
      },

      /** 消费待处理导航 - 获取并清除待处理导航操作，返回导航信息 */
      consumePendingNavigation: () => {
        const { pendingNavigation } = get();
        if (!pendingNavigation) return null;
        set({ pendingNavigation: null });
        return pendingNavigation;
      },

      /** 切换最大化 - 在最大化当前面板和恢复正常布局之间切换 */
      toggleMaximize: () => {
        const { activePaneId, maximizedPaneId } = get();
        if (maximizedPaneId) {
          set({ maximizedPaneId: null });
        } else {
          set({ maximizedPaneId: activePaneId });
        }
      },

      /** 重置面板 - 恢复默认的单面板布局，清除所有状态 */
      reset: () => {
        const newLeaf = createLeaf('/');
        set({
          root: newLeaf,
          activePaneId: newLeaf.id,
          maximizedPaneId: null,
          prefixKeyState: 'idle',
          prefixKeyTimer: null,
          pendingNavigation: null,
          pendingSplit: null,
          pendingSplitTimer: null,
        });
      },

      /** 设置待处理分屏 - 进入 pendingSplit 状态，等待用户输入导航键或超时执行 */
      setPendingSplit: (direction, position) => {
        const { prefixKeyTimer, pendingSplitTimer } = get();
        if (prefixKeyTimer) clearTimeout(prefixKeyTimer);
        if (pendingSplitTimer) clearTimeout(pendingSplitTimer);

        const timer = setTimeout(() => {
          get().executePendingSplit();
        }, 300);

        set({
          prefixKeyState: 'pendingSplit',
          prefixKeyTimer: null,
          pendingSplit: { direction, position },
          pendingSplitTimer: timer,
        });
      },

      /** 清除待处理分屏 - 取消分屏操作，恢复空闲状态 */
      clearPendingSplit: () => {
        const { pendingSplitTimer } = get();
        if (pendingSplitTimer) clearTimeout(pendingSplitTimer);
        set({ prefixKeyState: 'idle', prefixKeyTimer: null, pendingSplit: null, pendingSplitTimer: null });
      },

      /** 执行待处理分屏 - 完成分屏操作，可指定目标路由否则克隆当前页面 */
      executePendingSplit: (targetRoute?: string) => {
        const { pendingSplit, root, activePaneId, pendingSplitTimer } = get();
        if (!pendingSplit) return;

        const { direction, position } = pendingSplit;

        let route: string;
        let search: string;
        if (targetRoute) {
          route = targetRoute;
          search = '';
        } else {
          const activeNode = findNodeById(root, activePaneId);
          route = activeNode?.type === 'leaf' ? activeNode.route : '/';
          search = activeNode?.type === 'leaf' ? activeNode.search : '';
        }

        if (pendingSplitTimer) clearTimeout(pendingSplitTimer);

        if (leafCount(root) >= MAX_PANES) {
          set({ prefixKeyState: 'idle', prefixKeyTimer: null, pendingSplit: null, pendingSplitTimer: null });
          return;
        }

        const newRoot = splitLeaf(root, activePaneId, direction, position, route, search);
        if (newRoot === root) {
          set({ prefixKeyState: 'idle', prefixKeyTimer: null, pendingSplit: null, pendingSplitTimer: null });
          return;
        }

        const oldLeaves = new Set(getAllLeafIdsFromNode(root));
        const newLeaves = getAllLeafIdsFromNode(newRoot);
        const newLeafId = newLeaves.find(id => !oldLeaves.has(id));

        set({
          root: newRoot,
          activePaneId: newLeafId || activePaneId,
          prefixKeyState: 'idle',
          prefixKeyTimer: null,
          pendingSplit: null,
          pendingSplitTimer: null,
        });
      },

      /** 检查是否有多面板 - 返回是否存在多个面板的布尔值 */
      getHasMultiplePanes: () => {
        return hasMultiplePanes(get().root);
      },
    }),
    {
      name: 'autonomics-pane-storage',
      version: 1,
      migrate: (persistedState, version) => {
        if (version < 1 && persistedState && typeof persistedState === 'object') {
          const s = persistedState as { root?: PaneNode };
          if (s.root) {
            return { ...s, root: migrateLegacyRoutes(s.root) };
          }
        }
        return persistedState;
      },
      partialize: (state) => ({
        root: state.root,
        activePaneId: state.activePaneId,
        maximizedPaneId: state.maximizedPaneId,
      }),
    }
  )
);

// ========== 辅助函数 ==========

/**
 * 持久化迁移：将遗留的 /kms 路由重写为 /（KB 页已随知识库功能整体移除）。
 */
function migrateLegacyRoutes(root: PaneNode): PaneNode {
  if (root.type === 'leaf') {
    const route = root.route.startsWith('/kms') ? '/' : root.route;
    return { ...root, route };
  }
  return {
    ...root,
    first: migrateLegacyRoutes(root.first),
    second: migrateLegacyRoutes(root.second),
  };
}

/**
 * 递归获取面板树中所有叶子节点的 ID 列表
 * @param root - 面板树根节点
 * @returns 所有叶子节点的 ID 数组
 */
function getAllLeafIdsFromNode(root: PaneNode): PaneId[] {
  if (root.type === 'leaf') return [root.id];
  return [...getAllLeafIdsFromNode(root.first), ...getAllLeafIdsFromNode(root.second)];
}

/**
 * 递归替换分割节点的分割比例
 * @param root - 面板树根节点
 * @param splitNodeId - 目标分割节点 ID
 * @param ratio - 新的分割比例（0.15-0.85）
 * @returns 更新后的面板树
 */
function replaceSplitRatio(root: PaneNode, splitNodeId: PaneId, ratio: number): PaneNode {
  if (root.id === splitNodeId && root.type === 'split') {
    return { ...root, splitRatio: ratio };
  }
  if (root.type === 'split') {
    return {
      ...root,
      first: replaceSplitRatio(root.first, splitNodeId, ratio),
      second: replaceSplitRatio(root.second, splitNodeId, ratio),
    };
  }
  return root;
}

/**
 * 递归更新叶子节点的路由和搜索参数
 * @param root - 面板树根节点
 * @param leafId - 目标叶子节点 ID
 * @param route - 新的路由路径
 * @param search - 新的搜索参数字符串
 * @returns 更新后的面板树
 */
function updateLeafRoute(root: PaneNode, leafId: PaneId, route: string, search: string): PaneNode {
  if (root.id === leafId && root.type === 'leaf') {
    return { ...root, route, search };
  }
  if (root.type === 'split') {
    return {
      ...root,
      first: updateLeafRoute(root.first, leafId, route, search),
      second: updateLeafRoute(root.second, leafId, route, search),
    };
  }
  return root;
}

export default usePaneStore;
