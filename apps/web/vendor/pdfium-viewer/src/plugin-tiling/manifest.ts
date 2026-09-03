/**
 * @file 瓦片渲染插件 — 清单定义
 *
 * 本文件定义了瓦片渲染插件的元信息（manifest），包括：
 * - 插件 ID 与版本号
 * - 对外提供的能力标识（provides）
 * - 所依赖的其他插件（requires）
 * - 默认配置参数
 *
 * 设计决策：
 * - tileSize 默认 768px，在渲染质量和内存占用之间取得平衡
 * - overlapPx 默认 2.5px，用于消除瓦片拼接处的子像素缝隙
 * - extraRings 默认 0（不预渲染），避免不必要的渲染开销
 * - 依赖 render（渲染引擎）、scroll（滚动追踪）、viewport（视口度量）三个插件
 */

import { PluginManifest } from '../core';

import { TilingPluginConfig } from './types';

/** 插件的唯一标识符，用于在插件系统中注册和查找 */
export const TILING_PLUGIN_ID = 'tiling';

/**
 * 插件清单对象，描述插件的元数据和依赖关系。
 *
 * - provides: 该插件对外暴露的能力标识，其他插件/组件可通过 'tiling' 获取
 * - requires: 必须先于本插件加载的依赖插件列表
 * - optional: 可选依赖（当前无）
 * - defaultConfig: 若用户未提供配置，则使用此默认值
 */
export const manifest: PluginManifest<TilingPluginConfig> = {
  id: TILING_PLUGIN_ID,
  name: 'Tiling Plugin',
  version: '1.0.0',
  provides: ['tiling'],
  requires: ['render', 'scroll', 'viewport'],
  optional: [],
  defaultConfig: {
    tileSize: 768,
    overlapPx: 2.5,
    extraRings: 0,
  },
};
