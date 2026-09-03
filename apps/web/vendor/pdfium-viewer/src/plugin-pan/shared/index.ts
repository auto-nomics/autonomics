import { createPluginPackage } from '../../core';
import { PanPluginPackage as BasePanPackage } from '..';

import { PanMode } from './components';

export * from './hooks';
export * from './components';
export * from '..';

export const PanPluginPackage = createPluginPackage(BasePanPackage).addUtility(PanMode).build();
