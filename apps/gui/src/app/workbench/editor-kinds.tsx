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

import { lazy, Suspense, useCallback, useEffect } from "react";
import { ArrowLeft, ArrowRight, Bot, Compass, FileText, FolderCode, GitFork, Library, ListChecks, Network, NotebookText, Presentation, Rss, Server, Settings } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { BibliographyView } from "@/features/bibliography";
import { useBibliographySidebarSlot } from "@/features/bibliography/sidebar-slot";
import { registerBibRecordOpener } from "@/features/bibliography/bib-record-opener";
import { openBibliographyRecordEditor } from "./static-commands";
import { useMediaQuery } from "@/hooks/use-media-query";
import { WikiEntrySurface } from "@/features/wiki/wiki-editor";
import { WikiLocalGraph } from "./wiki-local-graph";
import {
  registerWikiOpener,
  resolveTitleCached,
} from "@/features/wiki/wiki-state";
import { DeckEditorBody, registerDeckOpener } from "@/features/deck";
import { getDeckDocument } from "@/features/deck/deck-document";
import { findWikiDocument } from "@/features/wiki/wiki-document";
import { boundedContextText, editorContextSnapshot } from "./editor-context-snapshots";
import { ProjectEditor } from "@/features/projects/project-editor";
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
  setContainerView,
  setSidebarCollapsed,
  closeEditor,
  allGroups,
  openWikiEntryEditor,
  openDeckEditor,
  patchEditorState,
  useWorkbenchState,
  type WikiEntryEditorState,
  type DeckEditorState,
} from "./workbench-store";
import { useActiveEditorContext } from "./active-editor-context";
import { useWorkbenchRuntime } from "./runtime-context";

// ---------------------------------------------------------------------------
// Bibliography / config / dag (adapted feature views)
// ---------------------------------------------------------------------------

function BibliographyEditorBody({ editor }: EditorContext) {
  const workspace: BibliographyWorkspace = ["library", "discover", "subscriptions", "manuscripts"].includes(String(editor.params.workspace))
    ? editor.params.workspace as BibliographyWorkspace
    : "library";
  const focusArticleId = workspace === "library" ? String(editor.params.articleId ?? "") || null : null;
  const focusNonce = workspace === "library" ? Number(editor.params.focusNonce ?? 0) || null : null;
  const groups = useWorkbenchState().groups;
  const sidebarSlot = useBibliographySidebarSlot();
  const desktop = useMediaQuery("(min-width: 768px)");
  const active = allGroups(groups).some(([, group]) => group.activeId === editor.id);
  const focused = useActiveEditorContext().editor?.id === editor.id;
  const externalSidebar = desktop || workspace === "subscriptions";
  useEffect(() => {
    if (!focused || workspace !== "manuscripts") return;
    const previous = getWorkbenchState().sidebar.collapsed;
    setSidebarCollapsed(true);
    return () => {
      if (getWorkbenchState().sidebar.collapsed) setSidebarCollapsed(previous);
    };
  }, [focused, workspace]);
  useEffect(() => {
    if (!focused || (workspace !== "library" && workspace !== "subscriptions")) return;
    setContainer("bibliography");
    setContainerView("bibliography", workspace);
  }, [focused, workspace]);
  return (
    <section aria-label="文献工作区" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <BibliographyView workspace={workspace} active={active} focusArticleId={focusArticleId} focusNonce={focusNonce} sidebarSlot={focused && externalSidebar ? sidebarSlot : null} externalSidebar={externalSidebar} onOpenSidebar={() => { setContainer("bibliography"); setSidebarCollapsed(false); activateEditorById(editor.id); }} />
    </section>
  );
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
  const focused = useActiveEditorContext().editor?.id === editor.id;
  useEffect(() => {
    if (focused) setContainer("agents");
  }, [focused]);
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
  const focused = useActiveEditorContext().editor?.id === editor.id;
  useEffect(() => {
    if (focused) setContainer("wiki");
  }, [focused]);
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
const LazyCanvasEditor = lazy(() => import("@/features/canvas/canvas-editor").then(module => ({ default: module.CanvasEditor })));
function CanvasEditorBody(context: EditorContext) {
  return <Suspense fallback={<p role="status" className="p-4 text-sm text-muted-foreground">加载画布编辑器…</p>}><LazyCanvasEditor {...context} /></Suspense>;
}

export function registerWorkbenchEditorKinds(): void {
  if (registered) return;
  registered = true;

  registerEditorKind({
    kind: "canvas", title: editor => (editor.state as { title?: string } | null)?.title || "未命名画布",
    body: CanvasEditorBody, mount: "keep-alive", hideMode: "visibility",
    resource: editor => `canvas:${editor.params.canvasId}`,
    agentContext: editor => ({
      project_id: String(editor.params.projectId ?? ""), figure_id: String(editor.params.canvasId),
      observed_revision: (editor.state as { revision?: number } | null)?.revision ?? 0,
      follow_up_inputs: JSON.stringify({ read: { action: "read", project_id: String(editor.params.projectId ?? ""), figure_id: String(editor.params.canvasId) } }),
      figureContract: "This canvas is associated with an illustrator specialist. The user's message is the editing instruction. Read the saved figure and revision, including open object annotations and sketch markups, before proposing changes. When dirty=true wait for canvas autosave, then read its current revision. Apply edits directly to the canvas using the current expected_revision. Continue with the returned revision. Preserve unrelated objects and provenance, and inspect the current canvas PNG. Read the selected IDs from selectedObjectIds/selectedObjects, which are bounded JSON strings; when selectionReady=false or selectedObjectsTruncated=true do not infer missing selection. Use only sketch markup IDs as markup_ids; object annotations are a separate review channel.",
      inspectedSvgPath: (editor.state as { inspectedSvgPath?: string | null } | null)?.inspectedSvgPath ?? null, svgPathContract: "inspectedSvgPath is bounded JSON from the live canvas. panel_visible=false means a previous inspection, not a current selection. Use it when the user refers to the path. Re-read current revision and SVG sha256/part_id before editing. Use bioicons parts/geometry/edit_parts to directly update that saved SVG; preserve other paths, objects and provenance. Continue with the returned revision and inspect the canvas PNG. Path IDs are graphical, not biological identities.", canvasId: String(editor.params.canvasId), projectId: String(editor.params.projectId ?? ""), revision: (editor.state as { revision?: number } | null)?.revision ?? 0, dirty: (editor.state as { dirty?: boolean } | null)?.dirty ?? false, selectedObjectIds: (editor.state as { selectedObjectIds?: string } | null)?.selectedObjectIds ?? "[]", selectedObjects: (editor.state as { selectedObjects?: string } | null)?.selectedObjects ?? "[]", selectionReady: (editor.state as { selectionReady?: boolean } | null)?.selectionReady ?? false, selectedObjectsTotal: (editor.state as { selectedObjectsTotal?: number } | null)?.selectedObjectsTotal ?? 0, selectedObjectsTruncated: (editor.state as { selectedObjectsTruncated?: boolean } | null)?.selectedObjectsTruncated ?? false, selectionContract: "selectedObjects is bounded JSON from the live canvas; wait for selectionReady=true, including unsaved changes when dirty=true. When truncated, ask for a narrower selection before whole-selection edits; reading the scene cannot recover omitted selection IDs. When dirty, wait for canvas autosave and read the current scene/revision before editing. inset_region/inset_callout are annotations, not scientific components/relations; unverified_inset_reference needs ownership reconciliation.", title: (editor.state as { title?: string } | null)?.title ?? "", objectCount: (editor.state as { objectCount?: number } | null)?.objectCount ?? 0 }),
    confirmLeave: editor => {
      const state = editor.state as { dirty?: boolean; saving?: boolean } | null;
      return Boolean(state?.saving) || Boolean(state?.dirty) && !window.confirm("画布有未保存的修改，关闭将丢弃修改。继续？");
    },
  });

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
      const entryId = current?.entryId ?? String(editor.params.entryId ?? "");
      const document = findWikiDocument(entryId);
      return { entryId, title: current?.title || state.title, mode: state.mode,
        ...(document ? boundedContextText(document.state.content) : {}),
        scope: document ? "知识词条当前内容，包含未保存的修改（最多 24000 字符）" : "知识词条标识与阅读状态",
      };
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
    body: WikiLocalGraph,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "bibliography",
    title: (editor) => ({ manuscripts: "LaTeX 手稿", discover: "发现", subscriptions: "订阅" }[String(editor.params.workspace)] ?? "文献与证据"),
    name: (editor) => ({ manuscripts: "Manuscripts", discover: "Discover", subscriptions: "Subscriptions" }[String(editor.params.workspace)] ?? "Bibliography"),
    icon: (editor) => ({ manuscripts: FileText, discover: Compass, subscriptions: Rss }[String(editor.params.workspace)] ?? Library),
    body: BibliographyEditorBody,
    mount: "keep-alive",
  });
  // Bibliography record opener bridge: agent bib:// links focus the library
  // tab on a record without spawning one tab per article.
  registerBibRecordOpener((spec) => {
    openBibliographyRecordEditor(spec.articleId);
  });
  registerEditorKind({
    kind: "plugin-projects",
    agentContext: (editor) => {
      const live = editorContextSnapshot(editor.id);
      if (live) return live;
      const state = editor.state as { project?: string; dirty?: boolean; containerVersion?: number } | null;
      return { project: state?.project ?? null, dirty: state?.dirty ?? false, containerVersion: state?.containerVersion ?? null, scope: "项目标识与编辑状态，不含文件正文" };
    },
    title: () => "插件管理",
    name: () => "Plugins",
    icon: () => FolderCode,
    body: ProjectEditor,
    mount: "keep-alive",
    confirmLeave: (editor) => Boolean((editor.state as { dirty?: boolean } | null)?.dirty) && !window.confirm("当前文件有未保存的修改，关闭将丢弃修改。继续？"),
  });
  registerEditorKind({
    kind: "vfs-file",
    agentContext: (editor) => editorContextSnapshot(editor.id) ?? { path: String(editor.params.path ?? ""), scope: "文件标识（内容尚未加载）" },
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

