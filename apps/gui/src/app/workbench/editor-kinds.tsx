/**
 * Editor kinds. Bibliography/config/dag are adapted feature views; the
 * agent and wiki-entry kinds are native workbench editors:
 *
 * - `agent:{path}` — the shell portals that agent's workspace into its
 *   registered slot, allowing multiple editor groups to show conversations.
 * - `wiki-entry:{entryId}` — a real keep-alive editor per entry with a
 *   per-tab history stack in `editor.state`; registers the wiki opener
 *   bridge so wiki UI can open tabs without importing the workbench.
 */

import { useCallback, useEffect } from "react";
import { ArrowLeft, ArrowRight, Bot, Camera, Compass, GitFork, Library, Network, NotebookText, Rss, Settings } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { BibliographyView } from "@/features/bibliography";
import { useBibliographySidebarSlot } from "@/features/bibliography/sidebar-slot";
import { useMediaQuery } from "@/hooks/use-media-query";
import { WikiEntrySurface } from "@/features/wiki/wiki-editor";
import { WikiGraph } from "@/features/wiki/wiki-graph";
import {
  registerWikiOpener,
  resolveTitleCached,
} from "@/features/wiki/wiki-state";
import { ConfigView, DagView } from "@/features/system";
import { leafName } from "@/features/agent";
import { registerEditorKind, type EditorContext } from "./editor-registry";
import { registerReaderEditorKind } from "./editors/reader-editor";
import { setAgentSlot } from "./agent-slot-registry";
import {
  activateEditorById,
  setContainer,
  setSidebarCollapsed,
  closeEditor,
  allGroups,
  getGroup,
  openWikiEntryEditor,
  patchEditorState,
  useWorkbenchState,
  type WikiEntryEditorState,
} from "./workbench-store";
import { useWorkbenchRuntime } from "./runtime-context";

// ---------------------------------------------------------------------------
// Bibliography / config / dag (adapted feature views)
// ---------------------------------------------------------------------------

function BibliographyEditorBody({ editor }: EditorContext) {
  const workspace = (editor.params.workspace as BibliographyWorkspace | undefined) ?? "library";
  const groups = useWorkbenchState().groups;
  const sidebarSlot = useBibliographySidebarSlot();
  const desktop = useMediaQuery("(min-width: 768px)");
  const active = allGroups(groups).some(([, group]) => group.activeId === editor.id);
  return (
    <section aria-label="文献工作区" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <BibliographyView workspace={workspace} active={active} sidebarSlot={desktop ? sidebarSlot : undefined} externalSidebar={desktop} onOpenSidebar={() => { setContainer("bibliography"); setSidebarCollapsed(false); activateEditorById(editor.id); }} />
    </section>
  );
}

function WikiGraphEditorBody() {
  const state = useWorkbenchState();
  const group = getGroup(state.groups, state.groups.active);
  const current = group?.editors.find(item => item.id === group.activeId && item.kind === "wiki-entry");
  const last = allGroups(state.groups).flatMap(([, group]) => group.editors).find(item => item.kind === "wiki-entry" && item.id === state.lastWikiEditorId);
  const editor = current ?? last;
  const wikiState = editor?.state as WikiEntryEditorState | null | undefined;
  const location = wikiState?.stack?.[wikiState.cursor];
  const entryId = location?.entryId ?? (editor ? String(editor.params.entryId ?? "") : "");
  return <WikiGraph entryId={entryId} title={location?.title || wikiState?.title || ""} />;
}

function ConfigEditorBody() {
  const runtime = useWorkbenchRuntime();
  return (
    <div className="min-h-0 flex-1 overflow-auto p-3">
      <ConfigView activeAgent={runtime.activeAgent} />
    </div>
  );
}

function DagEditorBody({ editor }: EditorContext) {
  const agentPath = String(editor.params.agentPath ?? "");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <DagView key={agentPath} activeAgent={agentPath} />
    </div>
  );
}

// ---------------------------------------------------------------------------
 // Agent editor: the slot div hosting the portaled workspace
// ---------------------------------------------------------------------------

function AgentEditorBody({ editor }: EditorContext) {
  const path = String(editor.params.path ?? "");
  // Stable ref callback: an inline closure would detach/attach on every
  // render and emit through the registry — force-re-rendering the whole
  // shell — forever.
  const setSlot = useCallback((node: HTMLDivElement | null) => setAgentSlot(path, node), [path]);
  // Each visible agent group receives its own portaled workspace.
  return (
    <div
      ref={setSlot}
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      data-agent-slot={path}
    />
  );
}

// ---------------------------------------------------------------------------
// Wiki entry editor: per-tab history stack in editor.state
// ---------------------------------------------------------------------------

function stateOf(editor: EditorContext["editor"]): WikiEntryEditorState {
  return (editor.state as WikiEntryEditorState | null) ?? {
    stack: [{ kind: "entry", entryId: String(editor.params.entryId ?? ""), title: "" }],
    cursor: 0,
    mode: "read",
    title: "",
  };
}

function WikiEntryEditorBody({ editor }: EditorContext) {
  const entryId = String(editor.params.entryId ?? "");
  const state = stateOf(editor);
  const current = state.stack[state.cursor] ?? { kind: "entry" as const, entryId, title: state.title };

  // Keep the tab title current: resolve once per entry id change.
  useEffect(() => {
    let cancelled = false;
    void resolveTitleCached(entryId).then((title) => {
      if (cancelled || !title) return;
      const snapshot = stateOf(editor);
      if (snapshot.title === title) return;
      patchEditorState(editor.id, { ...snapshot, title });
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entryId]);

  const push = useCallback((nextId: string, title?: string) => {
    if (nextId === current.entryId) return;
    const snapshot = stateOf(editor);
    const stack = [...snapshot.stack.slice(0, snapshot.cursor + 1), { kind: "entry" as const, entryId: nextId, title: title ?? "" }];
    patchEditorState(editor.id, { ...snapshot, stack, cursor: stack.length - 1, mode: "read" });
  }, [current.entryId, editor]);

  const step = useCallback((delta: -1 | 1) => {
    const snapshot = stateOf(editor);
    const next = snapshot.cursor + delta;
    if (next < 0 || next >= snapshot.stack.length) return;
    patchEditorState(editor.id, { ...snapshot, cursor: next, mode: "read" });
  }, [editor]);

  const setMode = useCallback((mode: "read" | "edit") => {
    const snapshot = stateOf(editor);
    patchEditorState(editor.id, { ...snapshot, mode });
  }, [editor]);

  const canBack = state.cursor > 0;
  const canForward = state.cursor < state.stack.length - 1;

  return (
    <section aria-label="知识库工作区" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <WikiEntrySurface
        navigation={<span className="flex shrink-0 items-center gap-0.5">
        <button
          type="button"
          aria-label="后退"
          title="后退"
          disabled={!canBack}
          onClick={() => step(-1)}
          className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
        >
          <ArrowLeft className="size-3.5" />
        </button>
        <button
          type="button"
          aria-label="前进"
          title="前进"
          disabled={!canForward}
          onClick={() => step(1)}
          className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
        >
          <ArrowRight className="size-3.5" />
        </button>
        </span>}
        key={current.entryId}
        entryId={current.entryId}
        mode={state.mode}
        onModeChange={setMode}
        onNavigate={(nextId, title, newTab) => {
          // Plain clicks push the current tab's history; ctrl/middle clicks
          // and dock rows with explicit new-tab open a separate editor.
          if (newTab) {
            openWikiEntryEditor(nextId, { title });
            return;
          }
          if (nextId === current.entryId) return; // self-link
          push(nextId, title);
        }}
        onDeleted={() => closeEditor(editor.id)}
      />
    </section>
  );
}

// ---------------------------------------------------------------------------
// Registration + bridges
// ---------------------------------------------------------------------------

let registered = false;

/** Idempotent registration; imported by the shell for side effects. */
export function registerWorkbenchEditorKinds(): void {
  if (registered) return;
  registered = true;

  // Wiki opener bridge: feature code calls openWikiEntry, we open tabs.
  registerWikiOpener((entryId, options) => {
    openWikiEntryEditor(entryId, { title: options.title, mode: options.mode, forceNew: options.forceNew });
  });

  registerEditorKind({
    kind: "agent",
    title: (editor) => leafName(String(editor.params.path ?? "")),
    // Long name doubles as the tab's title attribute (full path) so tests
    // and tooltips can target agents by path.
    name: (editor) => String(editor.params.path ?? ""),
    icon: () => Bot,
    body: AgentEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "wiki-entry",
    title: (editor) => {
      const state = editor.state as WikiEntryEditorState | null;
      const current = state?.stack[state.cursor];
      return current?.title || state?.title || "未命名词条";
    },
    name: (editor) => String(editor.params.entryId ?? ""),
    icon: () => NotebookText,
    body: WikiEntryEditorBody,
    // Keep-alive: unsaved block-editor drafts survive tab switches; the
    // confirmLeave veto guards closing while editing.
    mount: "keep-alive",
    confirmLeave: (editor) => {
      const state = editor.state as WikiEntryEditorState | null;
      if (state?.mode !== "edit") return false;
      return !window.confirm("该标签页正在编辑中,关闭将丢弃未保存的修改。继续?");
    },
    commands: (editor) => [
      { id: "wiki-entry.close", title: "Close wiki entry", run: () => closeEditor(editor.id) },
    ],
  });
  registerEditorKind({
    kind: "wiki-graph",
    title: () => "知识图谱",
    name: () => "知识关系图谱",
    icon: () => Network,
    body: WikiGraphEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "bibliography",
    title: (editor) => ({ discover: "发现", snapshots: "快照", subscriptions: "订阅" }[String(editor.params.workspace)] ?? "文献与证据"),
    name: (editor) => ({ discover: "Discover", snapshots: "Snapshots", subscriptions: "Subscriptions" }[String(editor.params.workspace)] ?? "Bibliography"),
    icon: (editor) => ({ discover: Compass, snapshots: Camera, subscriptions: Rss }[String(editor.params.workspace)] ?? Library),
    body: BibliographyEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "config",
    title: () => "设置",
    name: () => "Settings",
    icon: () => Settings,
    body: ConfigEditorBody,
    mount: "keep-alive",
  });
  // Reader editor kind lives in editors/reader-editor.tsx (registers its
  // opener bridge too).
  registerReaderEditorKind();

  registerEditorKind({
    kind: "dag",
    title: (editor) => `DAG · ${leafName(String(editor.params.agentPath ?? ""))}`,
    name: (editor) => `Agent DAG (${String(editor.params.agentPath ?? "")})`,
    icon: () => GitFork,
    body: DagEditorBody,
    mount: "keep-alive",
  });
}

