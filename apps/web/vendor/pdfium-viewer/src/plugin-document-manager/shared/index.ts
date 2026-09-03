import { createPluginPackage } from '../../core';
import { DocumentManagerPluginPackage as BaseDocumentManagerPackage } from '..';
import { FilePicker } from './components';

export * from './hooks';
export * from './components';
export * from '..';

// A convenience package that auto-registers our utilities
export const DocumentManagerPluginPackage = createPluginPackage(BaseDocumentManagerPackage)
  .addUtility(FilePicker) // headless utility consumers can mount once and call cap.openFileDialog()
  .build();
