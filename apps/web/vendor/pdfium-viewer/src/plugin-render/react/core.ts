/**
 * @file React 核心 hooks 重导出
 *
 * 本文件从插件系统的 core/react 模块重导出所有 React 相关的 hooks
 * （如 usePlugin、useCapability 等），使渲染插件的 React 层可以通过
 * 相对路径统一引用核心 hooks。
 *
 * 设计决策：
 * 作为 React 模块的 "核心桥梁"，将底层核心 hooks 与上层渲染插件的
 * React 组件解耦。若核心 hooks 的导出路径发生变化，只需修改此文件。
 */

/** 重导出核心模块提供的所有 React hooks 和工具函数 */
export * from '../../core/react';
