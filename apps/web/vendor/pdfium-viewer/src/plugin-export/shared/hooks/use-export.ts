import { useCapability, usePlugin } from '../../../core/shared';
import { ExportPlugin } from '../..';

export const useExportPlugin = () => usePlugin<ExportPlugin>(ExportPlugin.id);
export const useExportCapability = () => useCapability<ExportPlugin>(ExportPlugin.id);

/**
 * Hook for export capability for a specific document
 * @param documentId Document ID
 */
export const useExport = (documentId: string) => {
  const { provides } = useExportCapability();

  return {
    provides: provides?.forDocument(documentId) ?? null,
  };
};
