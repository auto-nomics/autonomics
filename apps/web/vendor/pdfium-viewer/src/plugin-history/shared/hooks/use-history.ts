import { useCapability, usePlugin } from '../../../core/shared';
import { HistoryPlugin } from '../..';

export const useHistoryPlugin = () => usePlugin<HistoryPlugin>(HistoryPlugin.id);
export const useHistoryCapability = () => useCapability<HistoryPlugin>(HistoryPlugin.id);
