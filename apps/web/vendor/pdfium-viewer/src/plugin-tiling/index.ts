/**
 * @file 瓦片渲染插件 — 入口模块
 *
 * 本文件是 plugin-tiling 插件包的统一入口点。
 * 它将插件的各个组成部分（清单、插件类、reducer、初始状态）组装为一个
 * PluginPackage 对象，供插件系统在启动时注册和加载。
 *
 * 同时通过 `export *` 将插件的核心类型、类和常量暴露给外部消费者。
 */

import { PluginPackage } from '../core';

import { TilingAction } from './actions';
import { manifest, TILING_PLUGIN_ID } from './manifest';
import { initialState, tilingReducer } from './reducer';
import { TilingPlugin } from './tiling-plugin';
import { TilingPluginConfig, TilingState } from './types';

/**
 * 瓦片渲染插件的完整包定义。
 *
 * 包含四个核心字段：
 * - manifest     — 插件元信息（ID、名称、版本、依赖等）
 * - create       — 工厂函数，根据注册表和配置创建 TilingPlugin 实例
 * - reducer      — Redux 风格的状态归约器，处理瓦片相关的 action
 * - initialState — 插件的初始状态树
 */
export const TilingPluginPackage: PluginPackage<
  TilingPlugin,
  TilingPluginConfig,
  TilingState,
  TilingAction
> = {
  manifest,
  create: (registry, config) => new TilingPlugin(TILING_PLUGIN_ID, registry, config),
  reducer: (state, action) => tilingReducer(state, action),
  initialState,
};

/** 导出插件核心类，供外部通过插件系统获取实例 */
export * from './tiling-plugin';
/** 导出所有类型定义，供外部使用 */
export * from './types';
/** 导出插件清单常量和 ID */
export * from './manifest';
