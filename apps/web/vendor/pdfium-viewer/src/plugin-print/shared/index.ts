import { createPluginPackage } from '../../core';
import { PrintPluginPackage as BasePrintPackage } from '..';

import { PrintFrame } from './components';

export * from './hooks';
export * from './components';
export * from '..';

export const PrintPluginPackage = createPluginPackage(BasePrintPackage)
  .addUtility(PrintFrame)
  .build();
