import { createPluginPackage } from '../../core';
import { AnnotationPluginPackage as BaseAnnotationPackage } from '..';
import { AnnotationRendererProvider } from './context/renderer-registry';
import { AnnotationNavigationHandler } from './components/annotation-navigation-handler';

export * from './hooks';
export * from './components';
export * from './components/types';
export * from './context';
export * from '..';

// Automatically wrap with AnnotationRendererProvider and navigation handler utility
export const AnnotationPluginPackage = createPluginPackage(BaseAnnotationPackage)
  .addWrapper(AnnotationRendererProvider)
  .addUtility(AnnotationNavigationHandler)
  .build();
