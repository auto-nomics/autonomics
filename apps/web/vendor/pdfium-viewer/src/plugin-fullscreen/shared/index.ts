import { createPluginPackage } from '../../core';
import { FullscreenPluginPackage as BaseFullscreenPackage } from '..';
import { FullscreenProvider } from './components';

export * from './hooks';
export * from './components';

export * from '..';

export const FullscreenPluginPackage = createPluginPackage(BaseFullscreenPackage)
  .addWrapper(FullscreenProvider)
  .build();
