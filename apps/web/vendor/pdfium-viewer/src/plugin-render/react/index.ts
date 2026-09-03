/**
 * @file 渲染插件 React 模块入口
 *
 * 作为 React 层的统一导出入口，将 shared 目录中的组件、hooks 和上层模块
 * 一次性导出，供外部 React 应用直接使用。
 *
 * 导出内容包含：
 * - RenderLayer 组件 —— 用于在 React 中渲染 PDF 页面
 * - useRenderPlugin / useRenderCapability 等 hooks
 * - 渲染插件的类型定义和核心类
 */

/** 导出共享模块（包含组件、hooks 和上层插件模块） */
export * from '../shared';
