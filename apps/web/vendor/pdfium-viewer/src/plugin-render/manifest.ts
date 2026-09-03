/**
 * @file 渲染插件清单（Manifest）
 *
 * 本文件定义了 Render Plugin 的静态元数据（清单），供插件系统在注册阶段读取。
 * 清单中声明了插件的唯一标识、名称、版本号、提供的能力（provides）、
 * 必须依赖的其他插件（requires）以及默认配置。
 *
 * 设计决策：
 * - 将清单与插件实现分离，便于插件系统在不实例化插件类的情况下完成依赖分析和拓扑排序。
 * - provides 声明为 ['render']，表示本插件向系统暴露 "render" 能力，其他插件可据此声明对本插件的依赖。
 * - requires 为空数组，表明渲染插件不依赖任何其他插件，可独立加载。
 */

import { PluginManifest } from '../core';
import { RenderPluginConfig } from './types';

/** 渲染插件的唯一标识符，用于在插件注册表中索引和查找 */
export const RENDER_PLUGIN_ID = 'render';

/**
 * 渲染插件的静态清单描述对象。
 * - id         – 插件唯一标识
 * - name       – 人类可读的插件名称
 * - version    – 语义化版本号
 * - provides   – 本插件向系统提供的能力列表，其他插件可通过此声明依赖
 * - requires   – 加载本插件前必须已加载的插件能力列表
 * - optional   – 可选依赖，若已加载则使用，否则降级
 * - defaultConfig – 插件的默认配置项，用户可在注册时覆盖
 */
export const manifest: PluginManifest<RenderPluginConfig> = {
  id: RENDER_PLUGIN_ID,
  name: 'Render Plugin',
  version: '1.0.0',
  provides: ['render'],
  requires: [],
  optional: [],
  defaultConfig: {},
};
