/**
 * Wiki module state shared by every wiki surface (sidebar browse view,
 * entry editors, the shell-mounted search dialog).
 *
 * Layering: features must not import the workbench store, so opening an
 * entry goes through the opener bridge — the workbench registers the real
 * implementation at startup, wiki code calls `openWikiEntry` (the old
 * `wiki:open-entry` window event, minus the globals).
 */

import { useSyncExternalStore } from "react";
import { toast } from "sonner";
import { getEntry, type WikiEntry } from "./wiki-api";

// ---------------------------------------------------------------------------
// Cross-tab entry cache (id → entry) and id → title completion cache.
// ---------------------------------------------------------------------------

export const entryCache = new Map<string, WikiEntry>();

export function cacheEntry(entry: WikiEntry): void {
  entryCache.set(entry.id, entry);
}

export async function fetchEntryCached(id: string): Promise<WikiEntry | null> {
  const cached = entryCache.get(id);
  if (cached) return cached;
  try {
    const entry = await getEntry(id);
    cacheEntry(entry);
    return entry;
  } catch (error) {
    toast.error((error as Error).message);
    return null;
  }
}

/** 静默解析 id → 标题(dock 行/tab 标题用):失败返回 null,不弹 toast。 */
const titleCache = new Map<string, string>();

export async function resolveTitleCached(id: string): Promise<string | null> {
  const cached = titleCache.get(id);
  if (cached) return cached;
  const cachedEntry = entryCache.get(id);
  if (cachedEntry) return cachedEntry.title;
  try {
    const entry = await getEntry(id);
    cacheEntry(entry);
    if (entry.title && entry.title !== id) titleCache.set(id, entry.title);
    return entry.title;
  } catch {
    // 词条已被净删/不在库 —— 保留 id 兜底显示。
    return null;
  }
}

export function cachedTitle(id: string): string | undefined {
  return titleCache.get(id) ?? entryCache.get(id)?.title;
}

// ---------------------------------------------------------------------------
 // Entry opener bridge (registered by the workbench shell).
// ---------------------------------------------------------------------------

export type WikiOpenOptions = {
  /** Open in a dedicated tab even if an editor for this entry exists. */
  forceNew?: boolean;
  /** Start in edit mode (new entries). */
  mode?: "read" | "edit";
  title?: string;
};

type WikiOpener = (entryId: string, options: WikiOpenOptions) => void;

let opener: WikiOpener | null = null;

/** The workbench registers the real opener (opens/activates an editor
 *  tab). Wiki UI calls `openWikiEntry` without knowing about tabs. */
export function registerWikiOpener(implementation: WikiOpener): void {
  opener = implementation;
}

export function openWikiEntry(entryId: string, options: WikiOpenOptions = {}): void {
  if (!opener) {
    toast.error("知识库尚未就绪");
    return;
  }
  opener(entryId, options);
}

// ---------------------------------------------------------------------------
// Search dialog visibility (Ctrl+K opens it from anywhere).
// ---------------------------------------------------------------------------

let searchOpen = false;
const searchListeners = new Set<() => void>();

export function subscribeWikiSearch(listener: () => void): () => void {
  searchListeners.add(listener);
  return () => {
    searchListeners.delete(listener);
  };
}

export function isWikiSearchOpen(): boolean {
  return searchOpen;
}

export function openWikiSearch(): void {
  if (searchOpen) return;
  searchOpen = true;
  for (const listener of searchListeners) listener();
}

export function closeWikiSearch(): void {
  if (!searchOpen) return;
  searchOpen = false;
  for (const listener of searchListeners) listener();
}

export function useWikiSearchOpen(): boolean {
  return useSyncExternalStore(subscribeWikiSearch, isWikiSearchOpen);
}

// ---------------------------------------------------------------------------
// Content revision counter: bumped on create/save/delete/restore/purge so
// the sidebar browse view (and any future listings) reload themselves.
// ---------------------------------------------------------------------------

let revision = 0;
const revisionListeners = new Set<() => void>();

export function subscribeWikiRevision(listener: () => void): () => void {
  revisionListeners.add(listener);
  return () => {
    revisionListeners.delete(listener);
  };
}

export function getWikiRevision(): number {
  return revision;
}

export function bumpWikiRevision(): void {
  revision += 1;
  for (const listener of revisionListeners) listener();
}

export function useWikiRevision(): number {
  return useSyncExternalStore(subscribeWikiRevision, getWikiRevision);
}
