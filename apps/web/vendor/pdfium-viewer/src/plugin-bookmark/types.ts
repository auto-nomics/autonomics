import { BasePluginConfig } from '../core';
import { Task } from '../models';
import { PdfBookmarkObject } from '../models';
import { PdfErrorReason } from '../models';

export interface BookmarkPluginConfig extends BasePluginConfig {}

// Scoped bookmark capability for a specific document
export interface BookmarkScope {
  getBookmarks(): Task<{ bookmarks: PdfBookmarkObject[] }, PdfErrorReason>;
}

export interface BookmarkCapability {
  // Active document operations
  getBookmarks: () => Task<{ bookmarks: PdfBookmarkObject[] }, PdfErrorReason>;

  // Document-scoped operations
  forDocument(documentId: string): BookmarkScope;
}
