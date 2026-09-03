/**
 * @file 渲染插件共享模块入口
 *
 * 统一导出渲染插件中可跨框架复用的共享资源，包括：
 * - components  – 渲染层 UI 组件（如 RenderLayer）
 * - hooks       – 渲染相关的 React hooks（如 useRender）
 * - 上层模块（..）– 插件类、类型定义等
 *
 * 设计决策：
 * 将组件和 hooks 放在 shared 目录下，未来若有 Vue/Svelte 等非 React 框架需求，
 * 可在此处做条件导出或添加框架适配层，保持 React 层与非 React 层的解耦。
 */

/** 导出所有共享 UI 组件 */
export * from './components';
/** 导出所有共享 hooks */
export * from './hooks';
/** 导出上层渲染插件模块（插件类、类型定义等） */
export * from '..';
