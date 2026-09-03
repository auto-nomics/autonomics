import { useCapability, usePlugin } from '../../../core/shared';
import { SelectionPlugin } from '../..';

export const useSelectionCapability = () => useCapability<SelectionPlugin>(SelectionPlugin.id);
export const useSelectionPlugin = () => usePlugin<SelectionPlugin>(SelectionPlugin.id);
