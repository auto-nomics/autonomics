/**
 * @file 瓦片渲染插件 — React Hooks
 *
 * 提供两个核心 hooks，用于在 React 组件中访问瓦片插件：
 *
 * - useTilingPlugin: 获取瓦片插件实例（包含完整的插件 API）
 * - useTilingCapability: 获取瓦片插件对外暴露的能力接口（推荐的访问方式）
 *
 * 设计决策：
 * 通过泛型参数将插件类型传入 usePlugin/useCapability，确保返回值具有完整的类型信息，
 * 消费者无需手动类型断言。
 */

import { useCapability, usePlugin } from '../../../core/shared';
import { TilingPlugin } from '../..';

/**
 * 获取瓦片插件实例的 hook。
 *
 * 返回插件的完整实例对象，包含所有内部方法和状态。
 * 通常用于需要直接操作插件实例的高级场景。
 *
 * @returns TilingPlugin 实例
 */
export const useTilingPlugin = () => usePlugin<TilingPlugin>(TilingPlugin.id);

/**
 * 获取瓦片插件能力接口的 hook。
 *
 * 返回 TilingCapability 对象，包含：
 * - renderTile: 渲染指定瓦片
 * - forDocument: 获取文档级别的操作 scope
 * - onTileRendering: 监听瓦片渲染状态变化
 *
 * 这是组件中访问瓦片功能的推荐方式，因为能力接口是插件系统的标准对外契约。
 *
 * @returns { provides: TilingCapability | undefined } 能力接口对象
 */
export const useTilingCapability = () => useCapability<TilingPlugin>(TilingPlugin.id);
