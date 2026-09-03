import { createPluginPackage } from '../../core';
import { ExportPluginPackage as BaseExportPackage } from '..';

import { Download } from './component';

export * from './hooks';
export * from './component';

export * from '..';

export const ExportPluginPackage = createPluginPackage(BaseExportPackage)
  .addUtility(Download)
  .build();
