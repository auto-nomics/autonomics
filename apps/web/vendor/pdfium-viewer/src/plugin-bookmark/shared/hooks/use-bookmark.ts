import { useCapability, usePlugin } from '../../../core/shared';
import { BookmarkPlugin } from '../..';

export const useBookmarkPlugin = () => usePlugin<BookmarkPlugin>(BookmarkPlugin.id);
export const useBookmarkCapability = () => useCapability<BookmarkPlugin>(BookmarkPlugin.id);
