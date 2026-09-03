import { createPluginPackage } from '../../core';
import { SelectionPluginPackage as BaseSelectionPluginPackage } from '..';

import { CopyToClipboard } from './components';

export * from './hooks';
export * from './components';
export * from './types';
export * from '..';

export const SelectionPluginPackage = createPluginPackage(BaseSelectionPluginPackage)
  .addUtility(CopyToClipboard)
  .build();
