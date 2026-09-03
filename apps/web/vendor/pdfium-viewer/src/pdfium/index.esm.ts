import createPdfium from './vendor/pdfium.js';
import type { PdfiumRuntimeMethods } from './vendor/runtime-methods';
import { createWrappedModule, type PdfiumModule, type WrappedPdfiumModule } from './base';

// Re-export all types from base
export type {
  PdfiumModule,
  PdfiumRuntimeMethods,
  Type,
  CWrappedFunc,
  NameToType,
  NamesToType,
  Functions,
  Wrapped,
  Methods,
  WrappedPdfiumModule,
} from './base';

export { DEFAULT_PDFIUM_WASM_URL } from './base';

export async function init(moduleOverrides: Partial<PdfiumModule>): Promise<WrappedPdfiumModule> {
  const pdfium = await createPdfium<PdfiumRuntimeMethods>(moduleOverrides);
  return createWrappedModule(pdfium);
}
