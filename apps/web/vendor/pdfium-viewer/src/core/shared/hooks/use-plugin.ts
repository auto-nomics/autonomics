import type { BasePlugin } from '../..';
import { useRegistry } from './use-registry';

type PluginState<T extends BasePlugin> = {
  plugin: T | null;
  isLoading: boolean;
  ready: Promise<void>;
};

/**
 * Hook to access a plugin.
 * @param pluginId The ID of the plugin to access
 * @returns The plugin or null during initialization
 * @example
 * // Get zoom plugin
 * const zoom = usePlugin<ZoomPlugin>(ZoomPlugin.id);
 */
export function usePlugin<T extends BasePlugin>(pluginId: T['id']): PluginState<T> {
  const { registry } = useRegistry();

  if (registry === null) {
    return {
      plugin: null,
      isLoading: true,
      ready: new Promise(() => {}),
    };
  }

  const plugin = registry.getPlugin<T>(pluginId);

  if (!plugin) {
    // 插件未注册或正在初始化，返回 null 让调用方优雅降级
    return {
      plugin: null,
      isLoading: false,
      ready: new Promise(() => {}),
    };
  }

  return {
    plugin,
    isLoading: false,
    ready: plugin.ready(),
  };
}
