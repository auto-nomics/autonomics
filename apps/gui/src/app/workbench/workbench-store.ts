/**
 * Workbench layout store: sidebar containers, editor groups/tabs, the
 * bottom panel. Agent chat and wiki entries live in the editor area like
 * every other kind — there is no separate auxiliary hosting anymore.
 *
 * Module-level external store in the theme.ts pattern: plain state, a
 * listener set, a referentially-stable snapshot consumed through
 * `useSyncExternalStore`. Layout persists to a single localStorage key
 * (`autonomics.workbench.v2`); the sidebar collapsed flag keeps its
 * pre-workbench key because the mobile drawer still reads it directly.
 */

import { useSyncExternalStore } from "react";

export type ContainerId = "bibliography" | "wiki" | "agents";
export type GroupId = "main" | "secondary";
export type EditorId = string;
export type PanelTab = "events" | "notices";

export type EditorRef = {
  id: EditorId;
  kind: string;
  /** Feature-defined editor input; `{ kind:key }` reuse patches it. */
  params: Record<string, unknown>;
  /** Feature-defined per-instance UI state (M2: wiki history stack). */
  state: unknown;
  pinned: boolean;
};

export type EditorGroup = { editors: EditorRef[]; activeId: EditorId | null };

export type WorkbenchState = {
  /** Bumped on every mutation so the snapshot identity always changes. */
  revision: number;
  sidebar: { collapsed: boolean; container: ContainerId | null; width: number };
  containerViews: Record<ContainerId, string>;
  panel: { visible: boolean; heightPercent: number; tab: PanelTab; unreadNotices: number };
  groups: { active: GroupId; main: EditorGroup; secondary: EditorGroup | null };
  /** Main-group share of the split layout, percent (25–75). */
  splitRatio: number;
};

const STORAGE_KEY = "autonomics.workbench.v2";
/** Pre-workbench key, still owned by this store (mobile drawer reads it). */
export const SIDEBAR_COLLAPSED_KEY = "autonomics.workbench.sidebar.collapsed";

/** Pre-M2 wiki tab persistence; migrated into editor tabs, then removed. */
const WIKI_TABS_KEY = "wiki.tabs.v1";

const PANEL_HEIGHT_DEFAULT = 30;

function clampPercent(value: number, min: number, max: number, fallback: number) {
  return Number.isFinite(value) && value >= min && value <= max ? value : fallback;
}

function isContainerId(value: unknown): value is ContainerId {
  return value === "bibliography" || value === "wiki" || value === "agents";
}

/** A wiki editor's per-instance state (history stack + cursor + mode). */
export type WikiEntryEditorState = {
  stack: Array<{ kind: "entry"; entryId: string; title: string }>;
  cursor: number;
  mode: "read" | "edit";
  title: string;
};

/** Stable editor id: `kind:key` with recursively sorted object keys, so the
 *  same logical editor opened with differently-ordered params reuses the tab. */
export function editorKey(params: Record<string, unknown>): string {
  const canonical = (value: unknown): string => {
    if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
    if (value && typeof value === "object") {
      const entries = Object.entries(value as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : 1));
      return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`).join(",")}}`;
    }
    return JSON.stringify(value) ?? "null";
  };
  return canonical(params);
}

function defaultState(): WorkbenchState {
  return {
    revision: 0,
    sidebar: { collapsed: readLegacySidebarCollapsed(), container: "bibliography", width: 220 },
    containerViews: { bibliography: "library", wiki: "browse", agents: "overview" },
    panel: { visible: false, heightPercent: PANEL_HEIGHT_DEFAULT, tab: "events", unreadNotices: 0 },
    groups: { active: "main", main: { editors: [], activeId: null }, secondary: null },
    splitRatio: 50,
  };
}

function readLegacySidebarCollapsed(): boolean {
  try {
    if (localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true") return true;
    // Mobile has no persistent rail; it starts with the drawer closed.
    return window.innerWidth < 768;
  } catch {
    return false;
  }
}

/** Migrates the pre-M2 wiki tab strip into editor tabs. Runs whenever the
 *  legacy key is still present; the key is removed afterwards. Locations
 *  on a tab's history stack that point at the (retired) home view are
 *  dropped, and a home-current tab activates the wiki browse view instead. */
function migrateWikiTabs(): {
  editors: EditorRef[];
  activeId: EditorId | null;
  browseActive: boolean;
} | null {
  try {
    const raw = localStorage.getItem(WIKI_TABS_KEY);
    if (!raw) return null;
    localStorage.removeItem(WIKI_TABS_KEY);
    const parsed = JSON.parse(raw) as {
      tabs?: Array<{
        id?: unknown;
        stack?: Array<{ kind?: unknown; entryId?: unknown; title?: unknown }>;
        cursor?: unknown;
        mode?: unknown;
      }>;
      activeId?: unknown;
    };
    if (!Array.isArray(parsed.tabs)) return null;
    const editors: EditorRef[] = [];
    let activeId: EditorId | null = null;
    let browseActive = false;
    for (const tab of parsed.tabs) {
      if (!Array.isArray(tab.stack) || tab.stack.length === 0) continue;
      const stack = tab.stack.filter(
        (loc): loc is { kind: "entry"; entryId: string; title: string } =>
          loc?.kind === "entry" && typeof loc.entryId === "string",
      );
      const cursorIndex = typeof tab.cursor === "number" ? tab.cursor : 0;
      if (!stack.length) {
        if (tab.id === parsed.activeId) browseActive = true;
        continue;
      }
      const cursor = Math.min(Math.max(0, cursorIndex), stack.length - 1);
      const current = stack[cursor];
      const id = `wiki-entry:${editorKey({ entryId: current.entryId })}`;
      const existing = editors.find((editor) => editor.id === id);
      if (!existing) {
        editors.push({
          id,
          kind: "wiki-entry",
          params: { entryId: current.entryId },
          state: { stack, cursor, mode: tab.mode === "edit" ? "edit" : "read", title: current.title ?? "" },
          pinned: false,
        });
      }
      if (tab.id === parsed.activeId) activeId = id;
    }
    if (!editors.length && !browseActive) return null;
    return { editors, activeId, browseActive };
  } catch {
    return null;
  }
}

function isEditorGroup(value: unknown): value is EditorGroup {
  return Boolean(value) && typeof value === "object"
    && Array.isArray((value as EditorGroup).editors)
    && typeof (value as EditorGroup).activeId === "string";
}

function reviveState(raw: unknown): WorkbenchState | null {
  if (!raw || typeof raw !== "object") return null;
  const source = raw as Partial<WorkbenchState> & { version?: number };
  if (source.version !== 2) return null;
  const base = defaultState();
  const state: WorkbenchState = {
    ...base,
    sidebar: {
      collapsed: base.sidebar.collapsed,
      width: clampPercent(source.sidebar?.width ?? NaN, 160, 600, 220),
      container: isContainerId(source.sidebar?.container) ? source.sidebar.container : base.sidebar.container,
    },
    containerViews: { ...base.containerViews, ...(source.containerViews ?? {}) },
    panel: {
      ...base.panel,
      ...(source.panel ?? {}),
      heightPercent: clampPercent(source.panel?.heightPercent ?? NaN, 15, 70, PANEL_HEIGHT_DEFAULT),
      unreadNotices: 0,
    },
    groups: {
      active: source.groups?.active === "secondary" && source.groups.secondary ? "secondary" : "main",
      main: isEditorGroup(source.groups?.main) ? source.groups.main : base.groups.main,
      secondary: isEditorGroup(source.groups?.secondary) ? source.groups.secondary : null,
    },
    splitRatio: clampPercent(source.splitRatio ?? NaN, 25, 75, 50),
  };
  if (state.groups.active === "secondary" && !state.groups.secondary) state.groups.active = "main";
  return state;
}

function loadState(): WorkbenchState {
  let loaded: WorkbenchState | null = null;
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      loaded = reviveState(JSON.parse(stored));
    }
  } catch {
    /* corrupt or unavailable storage: fall through to defaults */
  }
  let state = loaded ?? defaultState();
  // The legacy wiki tab strip folds in whenever it is still present —
  // covers upgrades from the panel era AND from the M1 transitional shell.
  const wiki = migrateWikiTabs();
  if (wiki) {
    state = {
      ...state,
      containerViews: { ...state.containerViews, wiki: wiki.browseActive ? "browse" : state.containerViews.wiki },
      groups: {
        ...state.groups,
        main: {
          editors: [...wiki.editors, ...state.groups.main.editors],
          activeId: wiki.activeId ?? state.groups.main.activeId,
        },
      },
    };
    if (wiki.activeId) state.groups.active = "main";
  }
  return state;
}

const listeners = new Set<() => void>();
let state: WorkbenchState = loadState();

function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      version: 2,
      sidebar: { container: state.sidebar.container, width: state.sidebar.width },
      containerViews: state.containerViews,
      panel: {
        visible: state.panel.visible,
        heightPercent: state.panel.heightPercent,
        tab: state.panel.tab,
      },
      groups: state.groups,
      splitRatio: state.splitRatio,
    }));
  } catch {
    /* storage unavailable — keep the in-memory layout */
  }
}

function commit(next: WorkbenchState) {
  state = { ...next, revision: next.revision + 1 };
  persist();
  for (const listener of listeners) listener();
}

export function subscribeWorkbench(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getWorkbenchState(): WorkbenchState {
  return state;
}

export function useWorkbenchState(): WorkbenchState {
  return useSyncExternalStore(subscribeWorkbench, getWorkbenchState);
}

// ---------------------------------------------------------------------------
// Editors
// ---------------------------------------------------------------------------

export type OpenEditorOptions = {
  kind: string;
  params?: Record<string, unknown>;
  state?: unknown;
  /** Defaults to the active group. */
  group?: GroupId;
  /** Defaults to true; false keeps the tab in the background. */
  activate?: boolean;
};

const EMPTY_PARAMS: Record<string, unknown> = {};

export function openEditor(options: OpenEditorOptions): EditorId {
  const params = options.params ?? EMPTY_PARAMS;
  const id = `${options.kind}:${editorKey(params)}`;
  const group: GroupId = options.group ?? state.groups.active;
  const base = state;

  // Reuse an existing tab in either group; otherwise append to the target.
  for (const groupId of ["main", "secondary"] as const) {
    const groupData = groupId === "main" ? base.groups.main : base.groups.secondary;
    if (!groupData) continue;
    const existing = groupData.editors.find((editor) => editor.id === id);
    if (!existing) continue;
    const updated: EditorGroup = {
      editors: groupData.editors.map((editor) => editor.id === id
        ? { ...editor, params: { ...editor.params, ...params } }
        : editor),
      activeId: options.activate === false ? groupData.activeId : id,
    };
    commit({
      ...base,
      groups: groupId === "main"
        ? { ...base.groups, main: updated, active: options.activate === false ? base.groups.active : "main" }
        : { ...base.groups, secondary: updated, active: options.activate === false ? base.groups.active : "secondary" },
    });
    return id;
  }

  const editor: EditorRef = { id, kind: options.kind, params, state: options.state ?? null, pinned: false };
  if (group === "secondary" && base.groups.secondary) {
    commit({
      ...base,
      groups: {
        ...base.groups,
        secondary: { editors: [...base.groups.secondary.editors, editor], activeId: options.activate === false ? base.groups.secondary.activeId : id },
        active: options.activate === false ? base.groups.active : "secondary",
      },
    });
  } else {
    commit({
      ...base,
      groups: {
        ...base.groups,
        main: { editors: [...base.groups.main.editors, editor], activeId: options.activate === false ? base.groups.main.activeId : id },
        active: options.activate === false ? base.groups.active : "main",
      },
    });
  }
  return id;
}

export function activateEditor(groupId: GroupId, id: EditorId): void {
  const groupData = groupId === "main" ? state.groups.main : state.groups.secondary;
  if (!groupData || !groupData.editors.some((editor) => editor.id === id)) return;
  commit({
    ...state,
    groups: {
      ...state.groups,
      active: groupId,
      [groupId]: { ...groupData, activeId: id },
    },
  });
}

/** Activates an editor wherever it lives (agent-tab sync and openers). */
export function activateEditorById(id: EditorId): void {
  if (state.groups.main.editors.some((editor) => editor.id === id)) {
    activateEditor("main", id);
  } else if (state.groups.secondary?.editors.some((editor) => editor.id === id)) {
    activateEditor("secondary", id);
  }
}

/** Editor id helpers for the kind-keyed tabs the shell syncs. */
export function agentEditorId(path: string): EditorId {
  return `agent:${editorKey({ path })}`;
}

export function wikiEntryEditorId(entryId: string): EditorId {
  return `wiki-entry:${editorKey({ entryId })}`;
}

export function findEditor(id: EditorId): EditorRef | undefined {
  return state.groups.main.editors.find((editor) => editor.id === id)
    ?? state.groups.secondary?.editors.find((editor) => editor.id === id);
}

export function openAgentEditor(path: string, activate = true): EditorId {
  return openEditor({ kind: "agent", params: { path }, activate });
}

export function openWikiEntryEditor(entryId: string, options: { title?: string; mode?: "read" | "edit"; forceNew?: boolean } = {}): EditorId {
  const id = wikiEntryEditorId(entryId);
  const existing = findEditor(id);
  if (existing && !options.forceNew) {
    if (options.mode || options.title) {
      const state = (existing.state as WikiEntryEditorState | null) ?? { stack: [{ kind: "entry", entryId, title: options.title ?? "" }], cursor: 0, mode: "read", title: options.title ?? "" };
      patchEditorState(id, {
        ...state,
        mode: options.mode ?? state.mode,
        title: options.title ?? state.title,
      });
    }
    activateEditorById(id);
    return id;
  }
  const stack = [{ kind: "entry" as const, entryId, title: options.title ?? "" }];
  return openEditor({
    kind: "wiki-entry",
    params: { entryId },
    state: { stack, cursor: 0, mode: options.mode ?? "read", title: options.title ?? "" },
    activate: true,
  });
}

export function setActiveGroup(groupId: GroupId): void {
  if (state.groups.active === groupId) return;
  if (groupId === "secondary" && !state.groups.secondary) return;
  commit({ ...state, groups: { ...state.groups, active: groupId } });
}

/** Closes an editor in either group. Returns false when the id is unknown. */
export function closeEditor(id: EditorId): boolean {
  const inMain = state.groups.main.editors.some((editor) => editor.id === id);
  const inSecondary = state.groups.secondary?.editors.some((editor) => editor.id === id);
  if (!inMain && !inSecondary) return false;
  const close = (groupData: EditorGroup): EditorGroup => {
    const index = groupData.editors.findIndex((editor) => editor.id === id);
    const editors = groupData.editors.filter((editor) => editor.id !== id);
    const nextActive = groupData.activeId !== id ? groupData.activeId : editors[Math.min(index, editors.length - 1)]?.id ?? null;
    return { editors, activeId: nextActive };
  };
  const main = inMain ? close(state.groups.main) : state.groups.main;
  const secondary = inSecondary && state.groups.secondary ? close(state.groups.secondary) : state.groups.secondary;
  const groups = { ...state.groups, main, secondary };
  if (secondary && !secondary.editors.length) {
    // Closing the last editor of the split folds it back into main.
    groups.secondary = null;
    groups.active = "main";
  }
  commit({ ...state, groups });
  return true;
}

export function closeOthers(groupId: GroupId, id: EditorId): void {
  const groupData = groupId === "main" ? state.groups.main : state.groups.secondary;
  if (!groupData) return;
  const kept = groupData.editors.find((editor) => editor.id === id);
  if (!kept) return;
  commit({
    ...state,
    groups: { ...state.groups, [groupId]: { editors: [kept], activeId: id } },
  });
}

export function setEditorPinned(id: EditorId, pinned: boolean): void {
  const patch = (groupData: EditorGroup): EditorGroup => ({
    ...groupData,
    editors: groupData.editors.map((editor) => editor.id === id ? { ...editor, pinned } : editor),
  });
  if (state.groups.main.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, main: patch(state.groups.main) } });
  } else if (state.groups.secondary?.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, secondary: patch(state.groups.secondary!) } });
  }
}

export function patchEditorParams(id: EditorId, patch: Record<string, unknown>): void {
  const apply = (groupData: EditorGroup): EditorGroup => ({
    ...groupData,
    editors: groupData.editors.map((editor) => editor.id === id
      ? { ...editor, params: { ...editor.params, ...patch } }
      : editor),
  });
  if (state.groups.main.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, main: apply(state.groups.main) } });
  } else if (state.groups.secondary?.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, secondary: apply(state.groups.secondary!) } });
  }
}

export function patchEditorState(id: EditorId, editorState: unknown): void {
  const apply = (groupData: EditorGroup): EditorGroup => ({
    ...groupData,
    editors: groupData.editors.map((editor) => editor.id === id ? { ...editor, state: editorState } : editor),
  });
  if (state.groups.main.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, main: apply(state.groups.main) } });
  } else if (state.groups.secondary?.editors.some((editor) => editor.id === id)) {
    commit({ ...state, groups: { ...state.groups, secondary: apply(state.groups.secondary!) } });
  }
}

/** Move an editor to an insertion position, preserving its complete state. */
export function moveEditor(from: GroupId, id: EditorId, to: GroupId, index: number): void {
  const source = state.groups[from];
  const target = state.groups[to];
  if (!source || (!target && to !== "secondary")) return;
  const sourceIndex = source.editors.findIndex((editor) => editor.id === id);
  if (sourceIndex < 0) return;
  const editor = source.editors[sourceIndex];
  const remaining = source.editors.filter((item) => item.id !== id);
  const destination = from === to ? remaining : (target?.editors ?? []);
  const insertion = Math.max(0, Math.min(destination.length, index - (from === to && sourceIndex < index ? 1 : 0)));
  const editors = [...destination.slice(0, insertion), editor, ...destination.slice(insertion)];
  const sourceActive = source.activeId === id ? remaining[Math.min(sourceIndex, remaining.length - 1)]?.id ?? null : source.activeId;
  const groups = { ...state.groups, [from]: { editors: remaining, activeId: sourceActive }, [to]: { editors, activeId: id }, active: to };
  if (from === "secondary" && to === "main" && !remaining.length) groups.secondary = null;
  commit({ ...state, groups });
}

/** Splits the editor area: the active editor of the active group moves to a
 *  fresh secondary group on the right (VSCode "split editor" with one tab). */
export function splitEditor(): void {
  if (state.groups.secondary) {
    // Split already open: just focus it.
    commit({ ...state, groups: { ...state.groups, active: "secondary" } });
    return;
  }
  const sourceGroup = state.groups.active === "secondary" ? state.groups.main : state.groups.main;
  const active = sourceGroup.activeId;
  if (!active) return;
  const moved = sourceGroup.editors.find((editor) => editor.id === active);
  if (!moved) return;
  const rest = sourceGroup.editors.filter((editor) => editor.id !== active);
  commit({
    ...state,
    groups: {
      active: "secondary",
      main: { editors: rest, activeId: rest.at(-1)?.id ?? null },
      secondary: { editors: [moved], activeId: active },
    },
  });
}

/** Folds the split: secondary editors move back to main (unless duplicated). */
export function closeSplit(): void {
  const secondary = state.groups.secondary;
  if (!secondary) return;
  const existing = new Set(state.groups.main.editors.map((editor) => editor.id));
  const merged = [...state.groups.main.editors];
  for (const editor of secondary.editors) {
    if (!existing.has(editor.id)) merged.push(editor);
  }
  const focusId = state.groups.main.activeId ?? merged.at(-1)?.id ?? null;
  commit({
    ...state,
    groups: {
      active: "main",
      main: { editors: merged, activeId: focusId },
      secondary: null,
    },
  });
}

export function setEditorSplitRatio(percentMain: number): void {
  const splitRatio = clampPercent(percentMain, 25, 75, 50);
  if (state.splitRatio === splitRatio) return;
  commit({ ...state, splitRatio });
}

// ---------------------------------------------------------------------------
// Sidebar / containers
// ---------------------------------------------------------------------------

export function setContainer(container: ContainerId): void {
  if (state.sidebar.container === container) return;
  commit({ ...state, sidebar: { ...state.sidebar, container }, containerViews: state.containerViews });
}

export function setContainerView(container: ContainerId, view: string): void {
  if (state.containerViews[container] === view) return;
  commit({ ...state, containerViews: { ...state.containerViews, [container]: view } });
}

export function setSidebarWidth(width: number): void {
  const next = Math.round(Math.max(160, Math.min(600, width)));
  if (!Number.isFinite(next) || state.sidebar.width === next) return;
  commit({ ...state, sidebar: { ...state.sidebar, width: next } });
}

export function setSidebarCollapsed(collapsed: boolean): void {
  if (state.sidebar.collapsed === collapsed) return;
  commit({ ...state, sidebar: { ...state.sidebar, collapsed } });
}

export function toggleSidebar(): void {
  setSidebarCollapsed(!state.sidebar.collapsed);
}

// ---------------------------------------------------------------------------
// Bottom panel
// ---------------------------------------------------------------------------

export function setPanelVisible(visible: boolean): void {
  if (state.panel.visible === visible) return;
  commit({ ...state, panel: { ...state.panel, visible, unreadNotices: visible && state.panel.tab === "notices" ? 0 : state.panel.unreadNotices } });
}

export function togglePanel(): void {
  setPanelVisible(!state.panel.visible);
}

export function setPanelTab(tab: PanelTab): void {
  const unreadNotices = tab === "notices" && state.panel.visible ? 0 : state.panel.unreadNotices;
  commit({ ...state, panel: { ...state.panel, tab, unreadNotices } });
}

export function setPanelHeight(percent: number): void {
  const heightPercent = clampPercent(percent, 15, 70, PANEL_HEIGHT_DEFAULT);
  if (state.panel.heightPercent === heightPercent) return;
  commit({ ...state, panel: { ...state.panel, heightPercent } });
}

/** Called by the shell when notices land while the notices tab is not in
 *  front; the panel tab and activity bar surface the badge. */
export function bumpUnreadNotices(count = 1): void {
  commit({ ...state, panel: { ...state.panel, unreadNotices: state.panel.unreadNotices + count } });
}
