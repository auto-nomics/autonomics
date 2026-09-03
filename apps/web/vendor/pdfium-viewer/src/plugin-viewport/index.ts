import { PluginPackage } from '../core';

import { ViewportAction } from './actions';
import { manifest, VIEWPORT_PLUGIN_ID } from './manifest';
import { viewportReducer, initialState } from './reducer';
import { ViewportPluginConfig, ViewportState } from './types';
import { ViewportPlugin } from './viewport-plugin';

export const ViewportPluginPackage: PluginPackage<
  ViewportPlugin,
  ViewportPluginConfig,
  ViewportState,
  ViewportAction
> = {
  manifest,
  create: (registry, config) => new ViewportPlugin(VIEWPORT_PLUGIN_ID, registry, config),
  reducer: viewportReducer,
  initialState: initialState,
};

export * from './viewport-plugin';
export * from './types';
export * from './manifest';
