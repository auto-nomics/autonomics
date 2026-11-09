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
import { ArrowLeft, ArrowRight, Bot, Camera, Compass, FileText, FlaskConical, FolderCode, GitFork, Library, ListChecks, Network, NotebookText, Presentation, Rss, Server, Settings } from "lucide-react";
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
import { DeckEditorBody, registerDeckOpener } from "@/features/deck";
import { getDeckDocument } from "@/features/deck/deck-document";
import { findWikiDocument } from "@/features/wiki/wiki-document";
import { ProjectEditor } from "@/features/projects/project-editor";
import { PluginManager } from "@/features/projects/plugin-manager";
import { PluginDevEditorBody } from "@/features/projects/plugin-dev-editor";
import { VfsFileEditorBody } from "@/features/vfs";
import { ConfigView, DagView, FlowView } from "@/features/system";
import { HpcJobsEditorView } from "@/features/hpc";
import { leafName } from "@/features/agent";
import { registerEditorKind, editorKind, type EditorContext } from "./editor-registry";
import { registerReaderEditorKind } from "./editors/reader-editor";
import { setAgentSlot } from "./agent-slot-registry";
import {
  getWorkbenchState,
  activateEditorById,
  setContainer,
  setSidebarCollapsed,
  closeEditor,
  allGroups,
  getGroup,
  openWikiEntryEditor,
  openDeckEditor,
  patchEditorState,
  useWorkbenchState,
  type WikiEntryEditorState,
  type DeckEditorState,
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

function DeckEditorBodyWrapper({ editor }: EditorContext) {
  const deckId = String(editor.params.deckId ?? "");
  const start = Number(editor.state && (editor.state as DeckEditorState).slideNo) || undefined;
  return (
    <DeckEditorBody
      deckId={deckId}
      startSlide={start}
      onSlideSeen={(no) => {
        const snapshot = (editor.state as DeckEditorState | null) ?? { title: "" };
        patchEditorState(editor.id, { ...snapshot, slideNo: no });
      }}
    />
  );
}

function ConfigEditorBody({ initialTab }: { initialTab?: string }) {
  const runtime = useWorkbenchRuntime();
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden p-3">
      <ConfigView activeAgent={runtime.activeAgent} initialTab={initialTab} />
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

function FlowEditorBody({ editor }: EditorContext) {
  const agentPath = String(editor.params.agentPath ?? "");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <FlowView key={agentPath} activeAgent={agentPath} />
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
    if (editorKind(editor.kind)?.confirmLeave?.(editor)) return;
    const snapshot = stateOf(editor);
    const stack = [...snapshot.stack.slice(0, snapshot.cursor + 1), { kind: "entry" as const, entryId: nextId, title: title ?? "" }];
    patchEditorState(editor.id, { ...snapshot, stack, cursor: stack.length - 1, mode: "read" });
  }, [current.entryId, editor]);

  const step = useCallback((delta: -1 | 1) => {
    const snapshot = stateOf(editor);
    const next = snapshot.cursor + delta;
    if (next < 0 || next >= snapshot.stack.length) return;
    if (editorKind(editor.kind)?.confirmLeave?.(editor)) return;
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
    hideMode: "visibility",
  });
  registerEditorKind({
    kind: "wiki-entry",
    resource: editor => { const state = stateOf(editor); return `wiki:${state.stack[state.cursor]?.entryId ?? editor.params.entryId}`; },
    supportsMultipleViews: true,
    agentContext: (editor) => {
      const state = stateOf(editor);
      const current = state.stack[state.cursor];
      return { entryId: current?.entryId ?? String(editor.params.entryId ?? ""), title: current?.title || state.title, mode: state.mode };
    },
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
      const id = state.stack[state.cursor]?.entryId ?? String(editor.params.entryId ?? "");
      const model = findWikiDocument(id);
      if (!model) return false;
      const otherViews = allGroups(getWorkbenchState().groups).flatMap(([, group]) => group.editors).some(item => {
        if (item.id === editor.id || item.kind !== "wiki-entry") return false;
        const other = item.state as WikiEntryEditorState | null;
        return (other?.stack[other.cursor]?.entryId ?? String(item.params.entryId ?? "")) === id && other?.mode === "edit";
      });
      if (otherViews) return false;
      if (model.state.saving) return true;
      if (!model.dirty) return false;
      if (!window.confirm("该标签页正在编辑中,关闭将丢弃未保存的修改。继续?")) return true;
      const entry = model.state.entry;
      model.update({ title: entry.title, alias: entry.alias.join(", "), tags: entry.tags.join(", "), content: entry.content });
      return false;
    },
    commands: (editor) => [
      { id: "wiki-entry.close", title: "Close wiki entry", run: () => closeEditor(editor.id) },
    ],
  });
  registerEditorKind({
    kind: "deck",
    resource: editor => `deck:${editor.params.deckId}`,
    supportsMultipleViews: true,
    confirmLeave: (editor) => {
      const id = String(editor.params.deckId ?? "");
      const others = allGroups(getWorkbenchState().groups).flatMap(([, group]) => group.editors).some(item => item.id !== editor.id && item.kind === "deck" && item.params.deckId === id);
      if (others) return false;
      const document = getDeckDocument(id);
      const snapshot = document.getSnapshot();
      if (snapshot.saving) return true;
      if (snapshot.source === null || snapshot.source === snapshot.savedSource) return false;
      if (!window.confirm("该文稿有未保存的修改，关闭将丢弃修改。继续？")) return true;
      document.discardSource();
      return false;
    },
    agentContext: (editor) => ({ deckId: String(editor.params.deckId ?? ""), title: (editor.state as DeckEditorState | null)?.title ?? "", slideNo: (editor.state as DeckEditorState | null)?.slideNo ?? null }),
    title: (editor) => {
      const state = editor.state as DeckEditorState | null;
      return state?.title || "未命名 deck";
    },
    name: (editor) => String(editor.params.deckId ?? ""),
    icon: () => Presentation,
    body: DeckEditorBodyWrapper,
    mount: "keep-alive",
    commands: (editor) => [
      { id: "deck.close", title: "Close deck", run: () => closeEditor(editor.id) },
    ],
  });
  // Deck opener bridge: features open deck tabs without the workbench.
  registerDeckOpener((deckId, options) => {
    openDeckEditor(deckId, { title: options.title, slideNo: options.slideNo, forceNew: options.forceNew });
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
    kind: "plugin-projects",
    agentContext: (editor) => {
      const state = editor.state as { project?: string; dirty?: boolean; containerVersion?: number } | null;
      return { project: state?.project ?? null, dirty: state?.dirty ?? false, containerVersion: state?.containerVersion ?? null, scope: "项目标识与编辑状态，不含文件正文" };
    },
    title: () => "插件项目",
    name: () => "Plugin projects",
    icon: () => FolderCode,
    body: ProjectEditor,
    mount: "keep-alive",
    confirmLeave: (editor) => Boolean((editor.state as { dirty?: boolean } | null)?.dirty) && !window.confirm("当前文件有未保存的修改，关闭将丢弃修改。继续？"),
  });
  registerEditorKind({
    kind: "plugin-manager",
    title: () => "插件管理",
    name: () => "Plugin manager",
    icon: () => FolderCode,
    body: PluginManager,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "plugin-dev",
    title: (editor) => `开发 · ${String(editor.params.name ?? "")}`,
    name: (editor) => `Plugin dev (${String(editor.params.name ?? "")})`,
    icon: () => FlaskConical,
    // keep-alive is load-bearing: the terminal session and unsaved file
    // edits survive tab switches.
    body: PluginDevEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "vfs-file",
    title: (editor) => {
      const path = String(editor.params.path ?? "");
      return path.split("/").filter(Boolean).pop() || path || "文件";
    },
    name: (editor) => String(editor.params.path ?? ""),
    icon: () => FileText,
    body: VfsFileEditorBody,
    // keep-alive: loaded contents and unsaved drafts survive tab
    // switches; the dirty veto guards closing with unsaved edits.
    mount: "keep-alive",
    confirmLeave: (editor) => Boolean((editor.state as { dirty?: boolean } | null)?.dirty) && !window.confirm("该文件有未保存的修改，关闭将丢弃修改。继续？"),
  });
  registerEditorKind({
    kind: "config",
    title: () => "设置",
    name: () => "Settings",
    icon: () => Settings,
    body: () => <ConfigEditorBody />,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "hpc-compute",
    title: () => "计算目标",
    name: () => "Compute targets",
    icon: () => Server,
    body: () => <ConfigEditorBody initialTab="compute" />,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "hpc-jobs",
    title: () => "HPC 任务",
    name: () => "HPC jobs",
    icon: () => ListChecks,
    body: () => <HpcJobsEditorView />,
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

  registerEditorKind({
    kind: "flow",
    title: (editor) => `Flow · ${leafName(String(editor.params.agentPath ?? ""))}`,
    name: (editor) => `Agent Flow (${String(editor.params.agentPath ?? "")})`,
    icon: () => Network,
    body: FlowEditorBody,
    mount: "keep-alive",
  });
}

