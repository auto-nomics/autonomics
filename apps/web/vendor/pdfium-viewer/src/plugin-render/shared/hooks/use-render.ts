/**
 * @file 渲染插件 React Hooks
 *
 * 本文件定义了两个与渲染插件交互的 React hooks：
 * - useRenderPlugin    – 获取渲染插件的完整实例（包含所有方法和内部状态）
 * - useRenderCapability – 获取渲染插件对外暴露的能力对象（推荐使用）
 *
 * 设计决策：
 * - 提供 Plugin 和 Capability 两层访问接口，遵循最小权限原则。
 *   大多数 UI 组件只需调用渲染方法，使用 useRenderCapability 即可；
 *   只有需要直接操作插件实例的场景（如读取配置、调用内部方法）才使用 useRenderPlugin。
 * - 两个 hook 均通过核心模块的通用 hook（usePlugin / useCapability）实现，
 *   并通过泛型参数指定为 RenderPlugin 类型，确保类型安全。
 */

import { useCapability, usePlugin } from '../../../core/shared';
import { RenderPlugin } from '../..';

/**
 * 获取渲染插件的完整实例。
 *
 * 返回的 RenderPlugin 实例包含所有公共和受保护的方法，
 * 适用于需要直接操作插件的高级场景。
 *
 * @returns RenderPlugin 实例
 *
 * @example
 * ```tsx
 * const plugin = useRenderPlugin();
 * // 可访问插件的所有方法
 * ```
 */
export const useRenderPlugin = () => usePlugin<RenderPlugin>(RenderPlugin.id);

/**
 * 获取渲染插件对外暴露的能力对象（RenderCapability）。
 *
 * 推荐大多数 UI 组件使用此 hook，它返回的对象仅包含渲染相关的公共方法：
 * - renderPage / renderPageRect – 渲染页面（返回 Blob）
 * - renderPageRaw / renderPageRectRaw – 渲染页面（返回原始像素）
 * - forDocument – 创建文档级渲染作用域
 *
 * @returns 包含 provides（RenderCapability）和 loading 状态的对象
 *
 * @example
 * ```tsx
 * const { provides } = useRenderCapability();
 * if (provides) {
 *   const task = provides.forDocument(docId).renderPage({ pageIndex: 0, options: {...} });
 * }
 * ```
 */
export const useRenderCapability = () => useCapability<RenderPlugin>(RenderPlugin.id);
