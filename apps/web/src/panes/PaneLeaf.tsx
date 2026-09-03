/**
 * 分栏叶子节点组件
 *
 * 渲染分栏树的终端节点（LeafNode），是分栏系统中实际承载内容的组件。
 * 每个 PaneLeaf 包含：
 * - 独立的 MemoryRouter：支持在同一页面中运行多个路由实例
 * - RouteSync 子组件：同步 pane 路由状态与全局 store
 * - 点击激活机制：点击 pane 时自动设置为活跃 pane
 * - 焦点管理：避免抢占可聚焦元素（INPUT、TEXTAREA 等）的焦点
 *
 * 设计说明：
 * - 使用 MemoryRouter 而非 BrowserRouter，实现每个 pane 独立的路由状态
 * - MemoryRouter 的 initialEntries 在挂载时冻结，后续路由变化由 pane 自行管理
 * - 通过 PaneIdContext.Provider 向子组件注入当前 pane ID
 * - 使用 React.memo 优化渲染性能，避免不必要的重渲染
 *
 * @module panes/PaneLeaf
 */
import React, { useEffect, useRef } from 'react';
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom';
import { LeafNode } from './types';
import usePaneStore from '../stores/usePaneStore';
import { PaneIdContext } from './PaneContext';
import PaneRoutes from './PaneRoutes';

/**
 * 路由同步组件
 *
 * 负责在 MemoryRouter 内部同步路由状态：
 * 1. 将当前 location 同步到 usePaneStore（供其他 pane 读取）
 * 2. 将活跃 pane 的路由同步到浏览器 URL 栏（仅 Web 模式，非 Tauri）
 * 3. 处理来自全局快捷键的待执行导航（如 Ctrl+B 后的面板切换）
 *
 * @param {object} props - 组件属性
 * @param {string} props.paneId - 当前 pane 的唯一标识符
 */
function RouteSync({ paneId }: { paneId: string }) {
  // 获取当前 MemoryRouter 的 location 对象
  const location = useLocation();
  // 获取 navigate 函数，用于程序化导航
  const navigate = useNavigate();
  // 从 store 获取当前活跃 pane ID
  const activePaneId = usePaneStore(s => s.activePaneId);
  // 判断当前 pane 是否为活跃 pane
  const isActive = activePaneId === paneId;
  // 从 store 获取待执行的导航操作
  const pendingNavigation = usePaneStore(s => s.pendingNavigation);

  /**
   * 将当前 location 同步到全局 store
   * 每次 location 变化时更新 store 中对应 pane 的路由状态
   */
  useEffect(() => {
    usePaneStore.getState().updatePaneRoute(paneId, location.pathname, location.search);
  }, [location, paneId]);

  /**
   * 将活跃 pane 的路由同步到浏览器 URL 栏
   * 仅在 Web 模式下生效（非 Tauri 桌面应用），
   * 确保浏览器地址栏显示当前活跃 pane 的路由路径
   */
  useEffect(() => {
    if (isActive && !('__TAURI_INTERNALS__' in window)) {
      window.history.replaceState(null, '', location.pathname + location.search);
    }
  }, [isActive, location]);

  /**
   * 处理来自全局快捷键的待执行导航
   * 当用户通过 Ctrl+B 前缀键切换面板时，store 中会设置 pendingNavigation，
   * 此 effect 检测到后执行实际的路由跳转
   */
  useEffect(() => {
    if (pendingNavigation && pendingNavigation.paneId === paneId) {
      // 消费待执行的导航操作（从 store 中读取并清除）
      const nav = usePaneStore.getState().consumePendingNavigation();
      if (nav) {
        navigate(nav.path); // 执行路由跳转
      }
    }
  }, [pendingNavigation, navigate, paneId]);

  // 此组件不渲染任何 UI，仅执行副作用
  return null;
}

/**
 * Pane 内容包装组件
 *
 * 为叶子节点提供 MemoryRouter 上下文和路由渲染。
 * MemoryRouter 的 initialEntries 在组件挂载时冻结，
 * 后续路由变化由 pane 内部的导航操作管理。
 *
 * @param {object} props - 组件属性
 * @param {LeafNode} props.leaf - 叶子节点数据，包含 id、route、search
 */
function PaneContent({ leaf }: { leaf: LeafNode }) {
  return (
    <MemoryRouter
      initialEntries={[leaf.route + (leaf.search || '')]} // 初始路由条目
      initialIndex={0} // 初始路由索引
      // 不能启用 v7_startTransition：它把 navigate 包进 React.startTransition，
      // 而点击事件会冒泡到 PaneLeaf.handleClick 触发 setActivePane（urgent update），
      // urgent update 会打断 pending transition → navigate 被静默丢弃，
      // 表现为"点了 PDF 但界面不跳"。v7_relativeSplatPath 与本 bug 无关，单独保留。
      future={{ v7_relativeSplatPath: true }}
    >
      <RouteSync paneId={leaf.id} /> {/* 路由同步组件 */}
      <PaneRoutes /> {/* 路由配置组件 */}
    </MemoryRouter>
  );
}

/**
 * PaneLeaf 组件的 Props 接口
 */
interface PaneLeafProps {
  /** 叶子节点数据，包含 id、route、search */
  leaf: LeafNode;
  /** 是否存在多个 pane（用于决定是否显示活跃/非活跃边框样式） */
  hasMultiplePanes: boolean;
}

/**
 * 分栏叶子节点组件（默认导出）
 *
 * 渲染分栏树的终端节点，提供以下功能：
 * - 点击激活：点击 pane 时将其设置为活跃 pane
 * - 焦点管理：点击非可聚焦区域时聚焦容器，使键盘事件正确路由
 * - 边框样式：多 pane 模式下显示活跃/非活跃边框区分
 * - Context 注入：通过 PaneIdContext.Provider 向子组件注入 pane ID
 *
 * 使用 React.memo 优化，避免在其他 pane 状态变化时重渲染。
 *
 * @param {PaneLeafProps} props - 组件属性
 * @returns {React.ReactElement} 分栏叶子节点的 JSX
 */
export default React.memo(function PaneLeaf({ leaf, hasMultiplePanes }: PaneLeafProps) {
  // 从 store 获取当前活跃 pane ID
  const activePaneId = usePaneStore(s => s.activePaneId);
  // 判断当前 pane 是否为活跃 pane
  const isActive = activePaneId === leaf.id;
  // 容器 DOM 引用，用于焦点管理
  const divRef = useRef<HTMLDivElement>(null);

  /**
   * 处理 pane 容器的点击事件
   *
   * 点击时执行两个操作：
   * 1. 将当前 pane 设置为活跃 pane（更新 store 状态）
   * 2. 聚焦容器 div（使键盘事件正确路由到当前 pane）
   *
   * 特殊处理：当点击目标已是可聚焦元素时（如 INPUT、TEXTAREA、contentEditable），
   * 不抢占焦点，避免导致输入框点击后焦点丢失的问题。
   */
  const handleClick = (e: React.MouseEvent) => {
    // 将当前 pane 设置为活跃 pane
    usePaneStore.getState().setActivePane(leaf.id);
    // 获取点击目标元素
    const target = e.target as HTMLElement;
    const tag = target.tagName;
    // 如果点击的是可聚焦元素，不抢占焦点
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (target.isContentEditable) return;
    // 聚焦容器 div，使键盘事件通过 document.activeElement 正确路由到当前 pane
    divRef.current?.focus();
  };

  return (
    <div
      ref={divRef} // 容器 DOM 引用
      onClick={handleClick} // 点击激活处理
      data-pane-id={leaf.id} // 数据属性，用于调试和测试
      tabIndex={-1} // 允许 div 接收焦点（但不参与 Tab 键导航）
      className={hasMultiplePanes
        ? (isActive ? 'pane-active-border' : 'pane-inactive-border') // 多 pane 模式：显示活跃/非活跃边框
        : '' // 单 pane 模式：无边框样式
      }
      style={{
        height: '100%', // 占满父容器高度
        width: '100%', // 占满父容器宽度
        overflow: 'hidden', // 隐藏溢出内容
        position: 'relative', // 相对定位，作为子元素的定位参考
        outline: 'none', // 移除焦点轮廓（使用自定义边框样式）
      }}
    >
      {/* 通过 Context 向子组件注入当前 pane ID */}
      <PaneIdContext.Provider value={leaf.id}>
        <PaneContent leaf={leaf} /> {/* 渲染 pane 内容 */}
      </PaneIdContext.Provider>
    </div>
  );
});
