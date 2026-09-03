import { useCapability, usePlugin } from '../../../core/shared';
import { ThumbnailPlugin } from '../..';

export const useThumbnailPlugin = () => usePlugin<ThumbnailPlugin>(ThumbnailPlugin.id);
export const useThumbnailCapability = () => useCapability<ThumbnailPlugin>(ThumbnailPlugin.id);
