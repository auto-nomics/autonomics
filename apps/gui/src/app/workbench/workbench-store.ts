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
import { editorKind } from "./editor-registry";
import { revealWorkbenchView } from "./view-layout";
import { layoutGroups, removeLayoutGroup, resizeLayout, splitLayout, validLayout, type DropDirection, type EditorLayout } from "./editor-layout";

export type ContainerId = string;
export type GroupId = string;
export type EditorId = string;
export type PanelTab = "events" | "notices" | "terminal";

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
  groups: { active: GroupId; main: EditorGroup; secondary: EditorGroup | null; extra: Record<GroupId, EditorGroup> };
  editorLayout: EditorLayout;
  /** Last focused wiki editor, retained while its graph tab is active. */
  lastWikiEditorId: EditorId | null;
  /** Most recently focused document that can be attached to an agent. */
  lastContextEditorId: EditorId | null;
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
  return typeof value === "string" && value.length > 0;
}

/** A wiki editor's per-instance state (history stack + cursor + mode). */
export type WikiEntryEditorState = {
  stack: Array<{ kind: "entry"; entryId: string; title: string }>;
  cursor: number;
  mode: "read" | "edit";
  title: string;
};

/** Per-tab deck state: last-viewed slide + resolved title. */
export type DeckEditorState = {
  title: string;
  slideNo?: number;
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
    containerViews: { bibliography: "library", wiki: "browse", agents: "list", projects: "files", vfs: "tree", decks: "browse" },
    panel: { visible: false, heightPercent: PANEL_HEIGHT_DEFAULT, tab: "events", unreadNotices: 0 },
    groups: { active: "main", main: { editors: [], activeId: null }, secondary: null, extra: {} },
    editorLayout: { group: "main" },
    lastWikiEditorId: null,
    lastContextEditorId: null,
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
    && (typeof (value as EditorGroup).activeId === "string" || (value as EditorGroup).activeId === null);
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
      active: typeof source.groups?.active === "string" ? source.groups.active : "main",
      main: isEditorGroup(source.groups?.main) ? source.groups.main : base.groups.main,
      secondary: isEditorGroup(source.groups?.secondary) ? source.groups.secondary : null,
      extra: Object.fromEntries(Object.entries(source.groups?.extra ?? {}).filter(([id, group]) => id !== "main" && id !== "secondary" && isEditorGroup(group))),
    },
    editorLayout: { group: "main" },
    lastWikiEditorId: typeof source.lastWikiEditorId === "string" ? source.lastWikiEditorId : null,
    lastContextEditorId: typeof source.lastContextEditorId === "string" ? source.lastContextEditorId : null,
    splitRatio: clampPercent(source.splitRatio ?? NaN, 25, 75, 50),
  };
  const ids = new Set(allGroups(state.groups).map(([id]) => id));
  state.editorLayout = validLayout(source.editorLayout, ids) ? source.editorLayout : [...ids].slice(1).reduce<EditorLayout>((layout, id) => ({ axis: "horizontal", ratio: state.splitRatio, first: layout, second: { group: id } }), { group: "main" });
  if (!getGroup(state.groups, state.groups.active)) state.groups.active = "main";
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
      editorLayout: state.editorLayout,
      lastWikiEditorId: state.lastWikiEditorId,
      lastContextEditorId: state.lastContextEditorId,
      splitRatio: state.splitRatio,
    }));
  } catch {
    /* storage unavailable — keep the in-memory layout */
  }
}

function commit(next: WorkbenchState) {
  const activeGroup = getGroup(next.groups, next.groups.active);
  const activeEditor = activeGroup?.editors.find(editor => editor.id === activeGroup.activeId);
  const editors = allGroups(next.groups).flatMap(([, group]) => group.editors);
  const lastWikiEditorId = activeEditor?.kind === "wiki-entry" ? activeEditor.id
    : editors.some(editor => editor.kind === "wiki-entry" && editor.id === next.lastWikiEditorId) ? next.lastWikiEditorId : null;
  const lastContextEditorId = activeEditor && editorKind(activeEditor.kind)?.agentContext ? activeEditor.id
    : editors.some(editor => editor.id === next.lastContextEditorId && editorKind(editor.kind)?.agentContext) ? next.lastContextEditorId : null;
  state = { ...next, lastWikiEditorId, lastContextEditorId, revision: next.revision + 1 };
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

export function allGroups(groups: WorkbenchState["groups"]): Array<[GroupId, EditorGroup]> {
  return [["main", groups.main], ...(groups.secondary ? [["secondary", groups.secondary] as [string, EditorGroup]] : []), ...Object.entries(groups.extra ?? {})];
}
export function getGroup(groups: WorkbenchState["groups"], id: GroupId): EditorGroup | null {
  return id === "main" ? groups.main : id === "secondary" ? groups.secondary : groups.extra[id] ?? null;
}
function putGroup(groups: WorkbenchState["groups"], id: GroupId, group: EditorGroup | null): WorkbenchState["groups"] {
  if (id === "main") return { ...groups, main: group ?? { editors: [], activeId: null } };
  if (id === "secondary") return { ...groups, secondary: group };
  const extra = { ...groups.extra }; if (group) extra[id] = group; else delete extra[id];
  return { ...groups, extra };
}

// ---------------------------------------------------------------------------
// Editors
// ---------------------------------------------------------------------------

export type OpenEditorOptions = {
  kind: string;
  /** Create another view instead of focusing the existing resource tab. */
  forceNew?: boolean;
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
  const id = options.forceNew ? `${options.kind}:view-${crypto.randomUUID()}` : `${options.kind}:${editorKey(params)}`;
  for (const [groupId, group] of allGroups(state.groups)) {
    const existing = group.editors.find(editor => editor.id === id);
    if (!existing) continue;
    const updated = { ...group, editors: group.editors.map(editor => editor.id === id ? { ...editor, params: { ...editor.params, ...params } } : editor), activeId: options.activate === false ? group.activeId : id };
    const groups = putGroup(state.groups, groupId, updated);
    commit({ ...state, groups: { ...groups, active: options.activate === false ? groups.active : groupId } });
    return id;
  }
  const groupId = getGroup(state.groups, options.group ?? state.groups.active) ? options.group ?? state.groups.active : "main";
  const group = getGroup(state.groups, groupId)!;
  const editor: EditorRef = { id, kind: options.kind, params, state: options.state ?? null, pinned: false };
  const groups = putGroup(state.groups, groupId, { editors: [...group.editors, editor], activeId: options.activate === false ? group.activeId : id });
  commit({ ...state, groups: { ...groups, active: options.activate === false ? groups.active : groupId } });
  return id;
}
export function activateEditor(groupId: GroupId, id: EditorId): void {
  const group = getGroup(state.groups, groupId);
  if (!group?.editors.some(editor => editor.id === id)) return;
  commit({ ...state, groups: { ...putGroup(state.groups, groupId, { ...group, activeId: id }), active: groupId } });
}
export function activateEditorById(id: EditorId): void {
  const found = allGroups(state.groups).find(([, group]) => group.editors.some(editor => editor.id === id));
  if (found) activateEditor(found[0], id);
}

/** Editor id helpers for the kind-keyed tabs the shell syncs. */
export function agentEditorId(path: string): EditorId {
  return `agent:${editorKey({ path })}`;
}

export function wikiEntryEditorId(entryId: string): EditorId {
  return `wiki-entry:${editorKey({ entryId })}`;
}

export function findEditor(id: EditorId): EditorRef | undefined {
  return allGroups(state.groups).flatMap(([, group]) => group.editors).find(editor => editor.id === id);
}

export function openAgentEditor(path: string, activate = true): EditorId {
  return openEditor({ kind: "agent", params: { path }, activate });
}

export function openWikiEntryEditor(entryId: string, options: { title?: string; mode?: "read" | "edit"; forceNew?: boolean } = {}): EditorId {
  const matches = (editor: EditorRef) => {
    if (editor.kind !== "wiki-entry") return false;
    const wiki = editor.state as WikiEntryEditorState | null;
    return (wiki?.stack[wiki.cursor]?.entryId ?? String(editor.params.entryId ?? "")) === entryId;
  };
  const existing = getGroup(state.groups, state.groups.active)?.editors.find(matches)
    ?? allGroups(state.groups).flatMap(([, group]) => group.editors).find(matches);
  const id = existing?.id ?? wikiEntryEditorId(entryId);
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
    forceNew: options.forceNew || Boolean(findEditor(wikiEntryEditorId(entryId)) && !existing),
    state: { stack, cursor: 0, mode: options.mode ?? "read", title: options.title ?? "" },
    activate: true,
  });
}

/** Stable id for a deck editor tab. */
export function deckEditorId(deckId: string): EditorId {
  return `deck:${editorKey({ deckId })}`;
}

/** Open (or focus) a deck tab; per-tab state keeps the last slide. */
export function openDeckEditor(deckId: string, options: { title?: string; slideNo?: number; forceNew?: boolean } = {}): EditorId {
  const canonicalId = `deck:${editorKey({ deckId })}`;
  const active = getGroup(state.groups, state.groups.active);
  const existing = active?.editors.find(editor => editor.kind === "deck" && editor.params.deckId === deckId)
    ?? findEditor(canonicalId)
    ?? allGroups(state.groups).flatMap(([, group]) => group.editors).find(editor => editor.kind === "deck" && editor.params.deckId === deckId);
  const id = existing?.id ?? canonicalId;
  if (existing && !options.forceNew) {
    if (options.title || options.slideNo) {
      const state = (existing.state as DeckEditorState | null) ?? { title: "" };
      patchEditorState(id, { ...state, title: options.title ?? state.title, slideNo: options.slideNo ?? state.slideNo });
    }
    activateEditorById(id);
    return id;
  }
  return openEditor({
    kind: "deck",
    forceNew: options.forceNew,
    params: { deckId },
    state: { title: options.title ?? "", slideNo: options.slideNo } satisfies DeckEditorState,
  });
}

export function setActiveGroup(groupId: GroupId): void {
  if (state.groups.active !== groupId && getGroup(state.groups, groupId)) commit({ ...state, groups: { ...state.groups, active: groupId } });
}
function removeEmptyGroup(groups: WorkbenchState["groups"], layout: EditorLayout, id: GroupId): { groups: WorkbenchState["groups"]; layout: EditorLayout } {
  if (allGroups(groups).length <= 1 || getGroup(groups, id)?.editors.length) return { groups, layout };
  // Keep the stable main group as an empty target; it is removed only by
  // promoting another group into it when no editor remains in main.
  if (id === "main") {
    const other = layoutGroups(layout).find(key => key !== "main");
    if (!other) return { groups, layout };
    const promoted = getGroup(groups, other)!;
    let next = putGroup(putGroup(groups, "main", promoted), other, null);
    if (next.active === other || next.active === "main") next = { ...next, active: "main" };
    const withoutMain = removeLayoutGroup(layout, "main")!;
    const rename = (node: EditorLayout): EditorLayout => "group" in node ? { group: node.group === other ? "main" : node.group } : { ...node, first: rename(node.first), second: rename(node.second) };
    return { groups: next, layout: rename(withoutMain) };
  }
  const nextLayout = removeLayoutGroup(layout, id) ?? { group: "main" };
  const next = putGroup(groups, id, null);
  return { groups: { ...next, active: next.active === id ? layoutGroups(nextLayout)[0] : next.active }, layout: nextLayout };
}
export function closeEditor(id: EditorId): boolean {
  const found = allGroups(state.groups).find(([, group]) => group.editors.some(editor => editor.id === id));
  if (!found) return false;
  const [groupId, group] = found, index = group.editors.findIndex(editor => editor.id === id);
  if (editorKind(group.editors[index].kind)?.confirmLeave?.(group.editors[index])) return false;
  const editors = group.editors.filter(editor => editor.id !== id);
  const updated = { editors, activeId: group.activeId === id ? editors[Math.min(index, editors.length - 1)]?.id ?? null : group.activeId };
  const result = removeEmptyGroup(putGroup(state.groups, groupId, updated), state.editorLayout, groupId);
  commit({ ...state, groups: result.groups, editorLayout: result.layout }); return true;
}
export function closeOthers(groupId: GroupId, id: EditorId): void {
  const group = getGroup(state.groups, groupId), kept = group?.editors.find(editor => editor.id === id);
  if (kept) for (const editor of group!.editors) {
    if (editor.id !== id) closeEditor(editor.id);
  }
}
function patchEditor(id: EditorId, update: (editor: EditorRef) => EditorRef): void {
  const found = allGroups(state.groups).find(([, group]) => group.editors.some(editor => editor.id === id));
  if (!found) return;
  const [groupId, group] = found;
  commit({ ...state, groups: putGroup(state.groups, groupId, { ...group, editors: group.editors.map(editor => editor.id === id ? update(editor) : editor) }) });
}
export function setEditorPinned(id: EditorId, pinned: boolean): void { patchEditor(id, editor => ({ ...editor, pinned })); }
export function patchEditorParams(id: EditorId, patch: Record<string, unknown>): void { patchEditor(id, editor => ({ ...editor, params: { ...editor.params, ...patch } })); }
export function patchEditorState(id: EditorId, editorState: unknown): void { patchEditor(id, editor => ({ ...editor, state: editorState })); }
export function moveEditor(from: GroupId, id: EditorId, to: GroupId, index: number): void {
  const source = getGroup(state.groups, from), target = getGroup(state.groups, to);
  if (!source || !target) return;
  const sourceIndex = source.editors.findIndex(editor => editor.id === id);
  if (sourceIndex < 0) return;
  const editor = source.editors[sourceIndex], remaining = source.editors.filter(item => item.id !== id);
  const destination = from === to ? remaining : target.editors;
  const insertion = Math.max(0, Math.min(destination.length, index - (from === to && sourceIndex < index ? 1 : 0)));
  const editors = [...destination.slice(0, insertion), editor, ...destination.slice(insertion)];
  const sourceActive = source.activeId === id ? remaining[Math.min(sourceIndex, remaining.length - 1)]?.id ?? null : source.activeId;
  let groups = putGroup(state.groups, from, { editors: remaining, activeId: sourceActive });
  groups = { ...putGroup(groups, to, { editors, activeId: id }), active: to };
  const result = from !== to ? removeEmptyGroup(groups, state.editorLayout, from) : { groups, layout: state.editorLayout };
  commit({ ...state, groups: result.groups, editorLayout: result.layout });
}
export function splitEditorAt(from: GroupId, id: EditorId, target: GroupId, direction: DropDirection): void {
  const source = getGroup(state.groups, from), destination = getGroup(state.groups, target);
  if (!source || !destination) return;
  const index = source.editors.findIndex(editor => editor.id === id);
  if (index < 0 || (from === target && source.editors.length === 1)) return;
  const newId = !state.groups.secondary ? "secondary" : `group-${crypto.randomUUID()}`;
  const editor = source.editors[index], rest = source.editors.filter(item => item.id !== id);
  let groups = putGroup(state.groups, from, { editors: rest, activeId: source.activeId === id ? rest[Math.min(index, rest.length - 1)]?.id ?? null : source.activeId });
  groups = { ...putGroup(groups, newId, { editors: [editor], activeId: id }), active: newId };
  let layout = splitLayout(state.editorLayout, target, newId, direction);
  if (from !== target) { const result = removeEmptyGroup(groups, layout, from); groups = result.groups; layout = result.layout; }
  commit({ ...state, groups, editorLayout: layout });
}
/** Reposition an entire group without touching its editors. */
export function moveGroupAt(from: GroupId, target: GroupId, direction: DropDirection): void {
  if (from === target || !getGroup(state.groups, from) || !getGroup(state.groups, target)) return;
  const remaining = removeLayoutGroup(state.editorLayout, from);
  if (!remaining) return;
  commit({ ...state, groups: { ...state.groups, active: from }, editorLayout: splitLayout(remaining, target, from, direction) });
}
/** Merge all tabs from one group into another at the requested position. */
export function mergeGroup(from: GroupId, target: GroupId, index: number): void {
  if (from === target) return;
  const source = getGroup(state.groups, from), dest = getGroup(state.groups, target);
  if (!source || !dest) return;
  const at = Math.max(0, Math.min(dest.editors.length, index));
  const editors = [...dest.editors.slice(0, at), ...source.editors, ...dest.editors.slice(at)];
  let groups = putGroup(state.groups, from, { editors: [], activeId: null });
  groups = { ...putGroup(groups, target, { editors, activeId: source.activeId ?? dest.activeId }), active: target };
  const result = removeEmptyGroup(groups, state.editorLayout, from);
  commit({ ...state, groups: result.groups, editorLayout: result.layout });
}
export function splitEditor(groupId: GroupId = state.groups.active): void {
  const group = getGroup(state.groups, groupId);
  const editor = group?.editors.find(item => item.id === group.activeId);
  if (editor && editorKind(editor.kind)?.supportsMultipleViews) {
    const newGroupId = !state.groups.secondary ? "secondary" : `group-${crypto.randomUUID()}`;
    const copy: EditorRef = { ...editor, id: `${editor.kind}:view-${crypto.randomUUID()}`, params: { ...editor.params }, state: structuredClone(editor.state), pinned: false };
    const groups = { ...putGroup(state.groups, newGroupId, { editors: [copy], activeId: copy.id }), active: newGroupId };
    commit({ ...state, groups, editorLayout: splitLayout(state.editorLayout, groupId, newGroupId, "right") });
    return;
  }
  if (group?.activeId) splitEditorAt(groupId, group.activeId, groupId, "right");
}
export function closeSplit(groupId: GroupId = state.groups.active): void {
  const ids = layoutGroups(state.editorLayout);
  if (ids.length < 2) return;
  const sourceId = ids.includes(groupId) ? groupId : ids.at(-1)!;
  const targetId = ids.find(id => id !== sourceId)!;
  const source = getGroup(state.groups, sourceId), target = getGroup(state.groups, targetId);
  if (!source || !target) return;
  const groups = { ...putGroup(putGroup(state.groups, sourceId, { editors: [], activeId: null }), targetId, { editors: [...target.editors, ...source.editors], activeId: target.activeId ?? source.activeId }), active: targetId };
  const result = removeEmptyGroup(groups, state.editorLayout, sourceId);
  commit({ ...state, groups: result.groups, editorLayout: result.layout });
}
export function setEditorLayoutRatio(path: string, ratio: number): void {
  if (!Number.isFinite(ratio) || ratio < 15 || ratio > 85) return;
  commit({ ...state, editorLayout: resizeLayout(state.editorLayout, path, ratio), splitRatio: path === "" ? ratio : state.splitRatio });
}
export function setEditorSplitRatio(percentMain: number): void { setEditorLayoutRatio("", percentMain); }

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
  commit({ ...state, panel: { ...state.panel, visible } });
}

export function togglePanel(): void {
  setPanelVisible(!state.panel.visible);
}

export function setPanelTab(tab: PanelTab): void {
  revealWorkbenchView(tab);
  commit({ ...state, panel: { ...state.panel, tab } });
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
export function clearUnreadNotices(): void {
  if (state.panel.unreadNotices) commit({ ...state, panel: { ...state.panel, unreadNotices: 0 } });
}
