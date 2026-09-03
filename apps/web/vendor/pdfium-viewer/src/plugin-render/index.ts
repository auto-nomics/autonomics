/**
 * @file 渲染插件包入口
 *
 * 本文件是 Render Plugin 的包入口，负责将插件的清单（manifest）、
 * 工厂函数、reducer 和初始状态打包为一个 PluginPackage 对象，
 * 供插件系统在注册阶段统一消费。
 *
 * 设计决策：
 * - create 采用工厂函数而非类实例，延迟插件实例的创建时机，
 *   使插件系统可以在依赖分析完成后再决定是否实例化。
 * - reducer 为空函数——渲染插件本身不维护独立的 Redux 状态，
 *   所有文档和页面状态由核心模块统一管理。
 * - initialState 为空对象，与空 reducer 保持一致。
 */

import { PluginPackage } from '../core';
import { RenderPluginConfig } from './types';
import { RenderPlugin } from './render-plugin';
import { manifest, RENDER_PLUGIN_ID } from './manifest';

/**
 * 渲染插件的完整包定义，包含：
 * - manifest     – 插件静态元数据（标识、版本、依赖等）
 * - create       – 插件实例工厂函数，在注册时由插件系统调用
 * - reducer      – 状态 reducer（空实现，渲染插件不维护独立状态）
 * - initialState – 初始状态（空对象）
 */
export const RenderPluginPackage: PluginPackage<RenderPlugin, RenderPluginConfig> = {
  manifest,
  create: (registry, config) => new RenderPlugin(RENDER_PLUGIN_ID, registry, config),
  reducer: () => {},
  initialState: {},
};

/** 导出渲染插件类，供外部直接引用或进行类型推断 */
export * from './render-plugin';
/** 导出所有类型定义，供依赖本插件的模块使用 */
export * from './types';
