import { createPluginPackage } from '../../core';
import { FormPluginPackage as BaseFormPackage } from '..';
import { FormRendererRegistration } from './components/form-renderer-registration';

export * from './hooks';
export * from './components';
export * from '..';

export const FormPluginPackage = createPluginPackage(BaseFormPackage)
  .addUtility(FormRendererRegistration)
  .build();
