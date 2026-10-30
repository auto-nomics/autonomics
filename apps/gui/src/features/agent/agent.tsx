import { DagMinimap } from "@/features/system/dag-minimap";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Layout, LayoutChangedMeta, PanelImperativeHandle } from "react-resizable-panels";
import {
  Archive,
  CheckCircle2,
  CircleDashed,
  CircleSlash,
  Copy,
  History,
  Hourglass,
  LoaderCircle,
  PenLine,
  RefreshCw,
  Shrink,
  Square,
  Wrench,
  XCircle,
} from "lucide-react";
import { gatewayRequest } from "@/gateway";
import type {
  AgentInfo,
  ChatItem,
  StoredAgent,
} from "@/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Switch } from "@/components/ui/switch";
import { layoutSideOpen, readAgentLayout, writeAgentLayout } from "./agent-layout";
import { cn } from "@/lib/utils";

export type DisplaySettings = {
  collapse_thinking: boolean;
  collapse_tool_calls: boolean;
  collapse_tool_results: boolean;
};

type View = "agent" | "config" | "bib";

type PickerItem = { id: string; role: "user" | "assistant"; text: string };

type Command = {
  id: string;
  title: string;
  category: string;
  keywords: string;
  run: () => void;
};

export function preview(text: string) {
  const line = text.split(/\r?\n/).find((item) => item.trim()) ?? "";
  const collapsed = line.split(/\s+/).filter(Boolean).join(" ");
  return collapsed.length <= 100 ? collapsed : `${collapsed.slice(0, 99)}…`;
}

export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return "Copied message";
  } catch (cause) {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const copied = document.execCommand("copy");
    area.remove();
    return copied ? "Copied message" : `Copy failed: ${String(cause)}`;
  }
}

export function leafName(path: string) {
  return path.split("/").filter(Boolean).pop() ?? "";
}

export type AgentViewProps = {
  activeAgent: string;
  agents: AgentInfo[];
  statuses: Record<string, string>;
  messages: ChatItem[];
  plan: { revision: number; update: any } | null;
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  promptHistory: string[];
  rightOpen: boolean;
  /** Desktop (≥ md): the inspector is a resizable panel. Below it it
   *  becomes a drawer, so the same flag opens a sheet instead. */
  isDesktop: boolean;
  onSetRight: (open: boolean) => void;
  /** Registers the center-column DOM node that hosts the (portaled)
   *  AgentWorkspace; called with null when this view unmounts. */
  onSlotChange: (slot: HTMLDivElement | null) => void;
  onSpawn: () => void;
  /** Restores a prompt from the history dialog into the workspace
   *  composer (the draft lives above both views). */
  onDraftChange: (value: string) => void;
  onCancel: () => void;
  onCompact: () => void;
  onShutdown: (path: string) => void;
  onSpawnAgent: (input: { name: string; parentPath: string; profile: string }) => Promise<void>;
  onResumeAgent: (agent: StoredAgent) => Promise<void>;
  onToggleDisplay: (key: keyof DisplaySettings) => void;
  onView: (view: View) => void;
  onOpenDag: () => void;
  onRefreshHistory: () => void;
};

/** Per-status icon + tone for agent lifecycle states. Backend source of
 *  truth is `AgentLifecycleStatus` (crates/agentik-types/src/lifecycle.rs);
 *  every variant must land in one of these arms — unknown values fall
 *  through to the idle marker. */
export function agentStatusIconMeta(status?: string) {
  const value = (status ?? "idle").toLowerCase();
  if (value.includes("error") || value.includes("fail")) return { Icon: XCircle, className: "text-destructive" };
  if (value.includes("retry")) return { Icon: RefreshCw, className: "animate-spin text-amber-400" };
  if (value.includes("tool")) return { Icon: Wrench, className: "animate-pulse text-amber-400" };
  if (value.includes("stream")) return { Icon: PenLine, className: "animate-pulse text-sky-400" };
  if (value.includes("request") || value === "running")
    return { Icon: LoaderCircle, className: "animate-spin text-sky-400" };
  if (value.includes("compact")) return { Icon: Shrink, className: "animate-pulse text-violet-400" };
  if (value.includes("waiting")) return { Icon: Hourglass, className: "text-violet-400" };
  if (value.includes("cancel") || value.includes("abort"))
    return { Icon: CircleSlash, className: "text-muted-foreground/70" };
  // Host-level coarse projection: the turn finished cleanly and the agent
  // rests until the next message (lifecycle itself never emits this).
  if (value.includes("completed")) return { Icon: CheckCircle2, className: "text-emerald-500/80" };
  return { Icon: CircleDashed, className: "text-muted-foreground/60" };
}

export function AgentStatusIcon({ status, className }: { status?: string; className?: string }) {
  const { Icon, className: tone } = agentStatusIconMeta(status);
  return <Icon aria-hidden className={cn("size-3 shrink-0 motion-reduce:animate-none", tone, className)} />;
}

function planStepIcon(status: unknown) {
  const value = String(status ?? "");
  if (value === "Completed") return <span className="text-emerald-500">✓</span>;
  if (value === "Cancelled") return <span className="text-destructive">✗</span>;
  if (value === "InProgress") return <span className="animate-pulse text-sky-400">◐</span>;
  return <span className="text-muted-foreground">•</span>;
}

export function AgentView(props: AgentViewProps) {
  const {
    activeAgent,
    agents,
    statuses,
    messages,
    plan,
    usage,
    display,
    promptHistory,
    rightOpen,
    isDesktop,
  } = props;

  const [resumeOpen, setResumeOpen] = useState(false);
  const [resumeQuery, setResumeQuery] = useState("");
  const [storedAgents, setStoredAgents] = useState<StoredAgent[]>([]);
  const [resumeBusy, setResumeBusy] = useState<string | null>(null);
  const [renameAgent, setRenameAgent] = useState<{ id: string; name: string } | null>(null);
  const [deleteAgent, setDeleteAgent] = useState<{ id: string; confirm: string } | null>(null);
  const [picker, setPicker] = useState<{ open: boolean; query: string }>({
    open: false,
    query: "",
  });
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [toast, setToast] = useState<string | null>(null);

  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const planSteps: any[] = plan?.update?.plan ?? [];
  const running = statusLabel(statuses[activeAgent]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setPaletteOpen(true);
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "r" && activeAgent) {
        event.preventDefault();
        setHistoryOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeAgent]);

  const showToast = (message: string) => {
    setToast(message);
    window.setTimeout(() => setToast((current) => current === message ? null : current), 2200);
  };

  const loadStoredAgents = async () => {
    try {
      setStoredAgents(await gatewayRequest("GET", "/storage/agents"));
    } catch (cause) {
      showToast(`Failed to load persisted agents: ${String(cause)}`);
    }
  };

  const openResume = () => {
    setResumeQuery("");
    setResumeOpen(true);
    void loadStoredAgents();
  };

  const openCopy = () => {
    if (!messages.some((message) => (message.role === "user" || message.role === "assistant") && message.text.trim())) {
      showToast("No text messages available");
      return;
    }
    setPicker({ open: true, query: "" });
  };

  const selectPickerItem = async (item: PickerItem) => {
    setPicker((current) => ({ ...current, open: false }));
    showToast(await copyText(item.text));
  };

  const pickerItems = useMemo(() => {
    return [...messages]
      .filter((message) => (message.role === "user" || message.role === "assistant") && message.text.trim())
      .reverse()
      .map((message) => ({ id: String(message.id), role: message.role as "user" | "assistant", text: message.text }));
  }, [messages]);

  const filteredPickerItems = useMemo(() => {
    const needle = picker.query.trim().toLowerCase();
    return pickerItems.filter((item) => !needle || `${item.role} ${item.text}`.toLowerCase().includes(needle));
  }, [picker.query, pickerItems]);

  const commands = useMemo<Command[]>(() => [
    { id: "new-agent", title: "New agent", category: "agent", keywords: "spawn create start profile", run: props.onSpawn },
    { id: "resume-agent", title: "Resume agent", category: "agent", keywords: "restore reconnect continue previous", run: openResume },
    { id: "history", title: "Search prompt history", category: "agent", keywords: "ctrl r recall previous prompt", run: () => setHistoryOpen(true) },
    { id: "cancel", title: "Cancel agent", category: "agent", keywords: "stop abort interrupt", run: props.onCancel },
    { id: "compact", title: "Compact conversation", category: "agent", keywords: "compress summarize context", run: props.onCompact },
    { id: "copy", title: "Copy message", category: "clipboard", keywords: "clipboard transcript text", run: openCopy },
    { id: "model", title: "Select model", category: "config", keywords: "model provider api key activate", run: () => props.onView("config" as View) },
    { id: "bibliography", title: "Bibliography library", category: "research", keywords: "papers articles fulltext collections", run: () => props.onView("bib" as View) },
    { id: "dag", title: "View DAG", category: "data", keywords: "graph pipeline topology node edge", run: () => { if (activeAgent) props.onOpenDag(); else showToast("Select an agent first"); } },
    { id: "thinking", title: "Toggle thinking blocks", category: "view", keywords: "collapse fold reasoning", run: () => props.onToggleDisplay("collapse_thinking") },
    { id: "tool-calls", title: "Toggle tool calls", category: "view", keywords: "collapse fold function call", run: () => props.onToggleDisplay("collapse_tool_calls") },
    { id: "tool-results", title: "Toggle tool results", category: "view", keywords: "collapse fold output", run: () => props.onToggleDisplay("collapse_tool_results") },
  ], [props, activeAgent, messages]);

  const filteredCommands = useMemo(() => {
    const needle = paletteQuery.trim().toLowerCase();
    return commands.filter((command) => !needle || `${command.title} ${command.category} ${command.keywords}`.toLowerCase().includes(needle));
  }, [commands, paletteQuery]);

  const filteredStored = useMemo(() => {
    const needle = resumeQuery.trim().toLowerCase();
    return storedAgents
      .filter((agent) => !needle || `${agent.name} ${agent.config_json.kind ?? ""}`.toLowerCase().includes(needle))
      .sort((left, right) => right.last_active - left.last_active);
  }, [resumeQuery, storedAgents]);

  const filteredHistory = useMemo(() => {
    const needle = historyQuery.trim().toLowerCase();
    return [...promptHistory].reverse().filter((item) => !needle || item.toLowerCase().includes(needle));
  }, [historyQuery, promptHistory]);

  // Desktop layout: the inspector's width persists to localStorage; the
  // open flag stays in App so the workspace tab-bar toggle drives both
  // the panel and the mobile drawer.
  const rightPanelRef = useRef<PanelImperativeHandle | null>(null);
  const [defaultLayout] = useState<Layout | null>(() => {
    const saved = readAgentLayout();
    if (!saved) return null;
    // Prune ids of retired panels (the agents sidebar was removed): the
    // panel group only honors a defaultLayout whose key count equals the
    // mounted panel count, so a stale key would silently discard the
    // whole saved layout.
    const panelIds = ["workspace", "inspector"];
    return Object.fromEntries(
      Object.entries(saved).filter(([id]) => panelIds.includes(id)),
    ) as Layout;
  });

  const handleLayoutChanged = useCallback((layout: Layout, meta: LayoutChangedMeta) => {
    writeAgentLayout(meta.requestedLayout ?? layout);
    props.onSetRight(layoutSideOpen(layout, "inspector"));
  }, [props]);

  // Mirror the shared open flag onto the collapsible desktop panel.
  useEffect(() => {
    if (!isDesktop) return;
    const panel = rightPanelRef.current;
    if (!panel) return;
    if (rightOpen) {
      if (panel.isCollapsed()) panel.expand();
    } else if (!panel.isCollapsed()) {
      panel.collapse();
    }
  }, [isDesktop, rightOpen]);

  const renderInspectorSidebar = () => (
    <ScrollArea className="min-h-0 flex-1" contentClassName="space-y-4 p-3">
      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Overview</h2>
        <Card className="p-2.5 text-xs">
          <div className="truncate" title={activeAgent}>{activeAgent || "No agent"}</div>
          <div className="mt-1.5 space-y-1 text-muted-foreground">
            <div className="flex justify-between gap-2">
              <span className="shrink-0">Model</span>
              <span className="truncate">{activeInfo?.model ?? "—"}</span>
            </div>
            <div className="flex justify-between gap-2">
              <span className="shrink-0">Status</span>
              <span className="truncate">{activeAgent ? running : "—"}</span>
            </div>
            {usage && (
              <div className="flex justify-between gap-2">
                <span className="shrink-0">Tokens</span>
                <span className="truncate">{usage.input} in / {usage.output} out</span>
              </div>
            )}
            {usage?.cache ? (
              <div className="flex justify-between gap-2">
                <span className="shrink-0">Cache</span>
                <span className="truncate">{usage.cache}</span>
              </div>
            ) : null}
          </div>
        </Card>
      </section>

      <section>
        <div className="mb-1.5 flex items-center justify-between">
          <h2 className="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">DAG</h2>
          <Button size="xs" variant="ghost" disabled={!activeAgent} onClick={props.onOpenDag}>Open</Button>
        </div>
        {rightOpen && activeAgent ? <DagMinimap key={activeAgent} activeAgent={activeAgent} /> : <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No agent selected</div>}
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Plan</h2>
        {planSteps.length ? (
          <div className="space-y-1">
            {planSteps.map((step, index) => (
              <div key={index} className="flex items-start gap-2 rounded-md border bg-card p-1.5 text-xs">
                <span className="mt-px shrink-0">{planStepIcon(step.status)}</span>
                <span className={cn("min-w-0", step.status === "Completed" && "text-muted-foreground line-through")}>
                  {step.step}
                </span>
              </div>
            ))}
          </div>
        ) : (
          <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No active plan</div>
        )}
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">View</h2>
        <Card size="sm" className="gap-0 py-0">
          <CardContent className="p-0">
            {([
              ["collapse_thinking", "Collapse thinking"],
              ["collapse_tool_calls", "Collapse tool calls"],
              ["collapse_tool_results", "Collapse tool results"],
            ] as const).map(([key, label], index) => (
              <Fragment key={key}>
                {index > 0 && <Separator />}
                <label className="flex w-full cursor-pointer items-center justify-between gap-2 px-2 py-1.5 text-sm hover:bg-accent">
                  <span>{label}</span>
                  <Switch size="sm" checked={display[key]} onCheckedChange={() => props.onToggleDisplay(key)} />
                </label>
              </Fragment>
            ))}
          </CardContent>
        </Card>
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Actions</h2>
        <Card size="sm" className="gap-0 py-0">
          <CardContent className="p-1.5">
            <div className="grid grid-cols-2 gap-1.5">
              <Button size="xs" variant="secondary" onClick={props.onCancel}><Square />Cancel</Button>
              <Button size="xs" variant="secondary" onClick={props.onCompact}><Archive />Compact</Button>
              <Button size="xs" variant="secondary" onClick={openCopy}><Copy />Copy</Button>
              <Button size="xs" variant="secondary" onClick={() => setHistoryOpen(true)}><History />History</Button>
              <Button size="xs" variant="secondary" onClick={props.onRefreshHistory}><RefreshCw />Refresh</Button>
            </div>
          </CardContent>
        </Card>
      </section>
    </ScrollArea>
  );

  const workspaceSlot = (className: string) => (
    <div
      ref={props.onSlotChange}
      className={cn("flex min-h-0 min-w-0 flex-col", className)}
    />
  );

  return (
    <section className={cn("flex min-h-0 flex-1", !isDesktop && "relative flex-col")}>
      {isDesktop ? (
        <ResizablePanelGroup
          orientation="horizontal"
          defaultLayout={defaultLayout ?? undefined}
          onLayoutChanged={handleLayoutChanged}
          className="min-h-0 min-w-0 flex-1"
        >
          <ResizablePanel id="workspace" minSize="30%" className="min-w-0">
            {/* The workspace (tab bar, transcript, composer) is portaled into
                this slot from App so the same component instance survives view
                switches as the floating panel. */}
            {workspaceSlot("h-full")}
          </ResizablePanel>
          <ResizableHandle aria-label="Resize inspector sidebar" />
          <ResizablePanel
            id="inspector"
            panelRef={rightPanelRef}
            defaultSize="280px"
            minSize="200px"
            maxSize="420px"
            collapsible
            collapsedSize="0%"
            className="bg-muted/50"
          >
            <aside className="flex h-full min-h-0 overflow-hidden border-l">
              {renderInspectorSidebar()}
            </aside>
          </ResizablePanel>
        </ResizablePanelGroup>
      ) : (
        <>
          {workspaceSlot("flex-1")}
          <Sheet open={rightOpen} onOpenChange={props.onSetRight}>
            <SheetContent side="right" className="w-80 max-w-[85vw] gap-0">
              <SheetHeader className="sr-only">
                <SheetTitle>Inspector</SheetTitle>
                <SheetDescription>运行详情</SheetDescription>
              </SheetHeader>
              {renderInspectorSidebar()}
              <div className="sr-only">关闭</div>
            </SheetContent>
          </Sheet>
        </>
      )}

      <Dialog open={resumeOpen} onOpenChange={setResumeOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>Resume persisted agent</DialogTitle>
            <DialogDescription>Restore a stored agent. Existing runtime instances are focused.</DialogDescription>
          </DialogHeader>
          <Input value={resumeQuery} placeholder="Filter by path or profile" onChange={(event) => setResumeQuery(event.target.value)} />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-2">
            {filteredStored.slice(0, 300).map((agent) => {
              const running = agents.find((item) => item.agent_id === agent.id);
              return (
                <Card key={agent.id} className="p-2">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">{agent.name}</div>
                      <div className="truncate text-xs text-muted-foreground">
                        {agent.config_json.kind ?? "researcher"} · {agent.config_json.preferred_model ?? "default model"} · {new Date(agent.last_active).toLocaleString()}
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-1">
                      <Button
                        size="xs"
                        disabled={resumeBusy === agent.id}
                        onClick={async () => {
                          setResumeBusy(agent.id);
                          await props.onResumeAgent(agent);
                          setResumeBusy(null);
                          setResumeOpen(false);
                        }}
                      >
                        {running ? "Focus" : resumeBusy === agent.id ? "Resuming…" : "Resume"}
                      </Button>
                      <Button
                        size="xs"
                        variant="secondary"
                        onClick={() => setRenameAgent(renameAgent?.id === agent.id ? null : { id: agent.id, name: leafName(agent.name) })}
                      >
                        Rename
                      </Button>
                      <Button size="xs" variant="destructive" onClick={() => setDeleteAgent({ id: agent.id, confirm: "" })}>Delete</Button>
                    </div>
                  </div>
                  {renameAgent?.id === agent.id && (
                    <form
                      className="mt-2 flex gap-1.5"
                      onSubmit={async (event) => {
                        event.preventDefault();
                        const segments = agent.name.split("/").filter(Boolean);
                        const nextPath = segments.length <= 1 ? `/root/${renameAgent.name}` : `/${[...segments.slice(0, -1), renameAgent.name].join("/")}`;
                        await gatewayRequest("PATCH", `/storage/agents/${encodeURIComponent(agent.id)}`, { path: nextPath });
                        setRenameAgent(null);
                        await loadStoredAgents();
                      }}
                    >
                      <Input value={renameAgent.name} onChange={(event) => setRenameAgent({ id: agent.id, name: event.target.value })} />
                      <Button type="submit" size="sm">Save</Button>
                    </form>
                  )}
                  {deleteAgent?.id === agent.id && (
                    <form
                      className="mt-2 flex gap-1.5"
                      onSubmit={async (event) => {
                        event.preventDefault();
                        if (deleteAgent.confirm.trim().toLowerCase() !== "yes") return;
                        await gatewayRequest("DELETE", `/storage/agents/${encodeURIComponent(agent.id)}`);
                        const runningAgent = agents.find((item) => item.agent_id === agent.id);
                        if (runningAgent) await props.onShutdown(runningAgent.path);
                        setDeleteAgent(null);
                        await loadStoredAgents();
                      }}
                    >
                      <Input value={deleteAgent.confirm} placeholder='type "yes"' onChange={(event) => setDeleteAgent({ id: agent.id, confirm: event.target.value })} />
                      <Button type="submit" size="sm" variant="destructive">Delete forever</Button>
                    </form>
                  )}
                </Card>
              );
            })}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      <Dialog open={picker.open} onOpenChange={(open) => setPicker((current) => ({ ...current, open }))}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Copy message</DialogTitle>
            <DialogDescription>Click a transcript message to copy it.</DialogDescription>
          </DialogHeader>
          <Input value={picker.query} placeholder="Filter messages" onChange={(event) => setPicker((current) => ({ ...current, query: event.target.value }))} />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
            {filteredPickerItems.map((item, index) => (
              <button
                key={item.id}
                type="button"
                onClick={() => void selectPickerItem(item)}
                className={cn("grid w-full grid-cols-[44px_minmax(0,1fr)] gap-2 rounded-md border p-2 text-left hover:bg-accent", index === 0 && "border-primary bg-accent")}
              >
                <span className="text-xs text-muted-foreground">{item.role === "user" ? "YOU" : "AI"}</span>
                <span className="truncate text-sm">{preview(item.text)}</span>
              </button>
            ))}
            {!filteredPickerItems.length && <div className="p-4 text-sm text-muted-foreground">No matching messages</div>}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      <Dialog open={historyOpen} onOpenChange={setHistoryOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Prompt history</DialogTitle>
            <DialogDescription>Select a previous prompt to restore it in the composer.</DialogDescription>
          </DialogHeader>
          <Input value={historyQuery} placeholder="Filter prompts" onChange={(event) => setHistoryQuery(event.target.value)} />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
            {filteredHistory.map((item, index) => (
              <button
                key={`${index}-${item}`}
                type="button"
                onClick={() => {
                  props.onDraftChange(item);
                  setHistoryOpen(false);
                }}
                className="w-full truncate rounded-md border p-2 text-left text-sm hover:bg-accent"
              >
                {preview(item)}
              </button>
            ))}
            {!filteredHistory.length && <div className="p-4 text-sm text-muted-foreground">No matching prompts</div>}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      <Dialog open={paletteOpen} onOpenChange={setPaletteOpen}>
        <DialogContent className="sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>Command palette</DialogTitle>
            <DialogDescription>Ctrl+P opens · type to filter · Enter runs.</DialogDescription>
          </DialogHeader>
          <Input
            value={paletteQuery}
            placeholder="Search commands"
            autoFocus
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                filteredCommands[0]?.run();
                setPaletteOpen(false);
              }
            }}
            onChange={(event) => setPaletteQuery(event.target.value)}
          />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
            {filteredCommands.map((command) => (
              <button
                key={command.id}
                type="button"
                onClick={() => {
                  command.run();
                  setPaletteOpen(false);
                }}
                className="grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md border p-2 text-left hover:bg-accent"
              >
                <span className="truncate text-sm">{command.title}</span>
                <Badge variant="outline">{command.category}</Badge>
              </button>
            ))}
            {!filteredCommands.length && <div className="p-4 text-sm text-muted-foreground">No matching commands</div>}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      {toast && (
        <div className="fixed bottom-5 left-1/2 z-[120] -translate-x-1/2 rounded-md border border-primary/40 bg-popover px-3 py-2 text-sm shadow-lg">
          {toast}
        </div>
      )}
    </section>
  );
}

export function statusLabel(status?: string) {
  return (status ?? "idle").toLowerCase();
}

/** Spawn form lifted out of AgentView so the workspace tab bar can open
 *  it while floating over another view, where AgentView is unmounted. */
export function SpawnAgentDialog({
  open,
  onOpenChange,
  busy,
  onSpawnAgent,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  busy: string | null;
  onSpawnAgent: (input: { name: string; parentPath: string; profile: string }) => Promise<void>;
}) {
  const [spawnName, setSpawnName] = useState("");
  const [spawnParent, setSpawnParent] = useState("/root");
  const [spawnProfile, setSpawnProfile] = useState("researcher");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New agent</DialogTitle>
          <DialogDescription>Spawn a resident agent under an existing path.</DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={async (event) => {
            event.preventDefault();
            await onSpawnAgent({ name: spawnName, parentPath: spawnParent, profile: spawnProfile });
            onOpenChange(false);
          }}
        >
          <Input value={spawnName} placeholder="agent name" onChange={(event) => setSpawnName(event.target.value)} required />
          <Input value={spawnParent} placeholder="/root" onChange={(event) => setSpawnParent(event.target.value)} required />
          <Input value={spawnProfile} placeholder="profile" onChange={(event) => setSpawnProfile(event.target.value)} required />
          <Button type="submit" disabled={busy === "spawn"}>Spawn</Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
