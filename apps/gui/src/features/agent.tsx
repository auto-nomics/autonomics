import { useEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { renderMarkdown } from "@/markdown";
import type {
  AgentInfo,
  BackendMessage,
  ChatItem,
  SessionInfo,
  StoredAgent,
} from "@/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

export type DisplaySettings = {
  collapse_thinking: boolean;
  collapse_tool_calls: boolean;
  collapse_tool_results: boolean;
};

type View = "agent" | "config" | "bib" | "kms" | "dag";

type PickerMode = "copy" | "fork";
type PickerItem = { id: string; role: "user" | "assistant"; text: string };

type Command = {
  id: string;
  title: string;
  category: string;
  keywords: string;
  run: () => void;
};

function preview(text: string) {
  const line = text.split(/\r?\n/).find((item) => item.trim()) ?? "";
  const collapsed = line.split(/\s+/).filter(Boolean).join(" ");
  return collapsed.length <= 100 ? collapsed : `${collapsed.slice(0, 99)}…`;
}

async function copyText(text: string) {
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

function leafName(path: string) {
  return path.split("/").filter(Boolean).pop() ?? "";
}

export type AgentViewProps = {
  activeAgent: string;
  activeSessionId: string | null;
  agents: AgentInfo[];
  sessions: SessionInfo[];
  statuses: Record<string, string>;
  messages: ChatItem[];
  streaming: ChatItem | null;
  plan: { revision: number; update: any } | null;
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  promptHistory: string[];
  rawMessages: BackendMessage[];
  draft: string;
  busy: string | null;
  onDraftChange: (value: string) => void;
  onSelectAgent: (path: string) => void;
  onSendMessage: () => void;
  onCancel: () => void;
  onCompact: () => void;
  onShutdown: (path: string) => void;
  onDelete: (path: string, agentId: string | null) => void;
  onSpawnAgent: (input: { name: string; parentPath: string; profile: string }) => Promise<void>;
  onResumeAgent: (agent: StoredAgent) => Promise<void>;
  onCreateSession: (title: string) => Promise<void>;
  onRenameSession: (sessionId: string, title: string) => Promise<void>;
  onCloseSession: (sessionId: string) => Promise<void>;
  onActivateSession: (sessionId: string) => Promise<void>;
  onForkMessage: (message: PickerItem) => Promise<void>;
  onToggleDisplay: (key: keyof DisplaySettings) => void;
  onView: (view: View) => void;
  onRefreshHistory: () => void;
};

export function AgentView(props: AgentViewProps) {
  const {
    activeAgent,
    activeSessionId,
    agents,
    sessions,
    statuses,
    messages,
    streaming,
    plan,
    usage,
    display,
    promptHistory,
    rawMessages,
    draft,
    busy,
  } = props;

  const [spawnOpen, setSpawnOpen] = useState(false);
  const [spawnName, setSpawnName] = useState("");
  const [spawnParent, setSpawnParent] = useState("/root");
  const [spawnProfile, setSpawnProfile] = useState("researcher");
  const [sessionCreating, setSessionCreating] = useState(false);
  const [sessionTitle, setSessionTitle] = useState("");
  const [sessionEditor, setSessionEditor] = useState<{ id: string; title: string } | null>(null);
  const [resumeOpen, setResumeOpen] = useState(false);
  const [resumeQuery, setResumeQuery] = useState("");
  const [storedAgents, setStoredAgents] = useState<StoredAgent[]>([]);
  const [resumeBusy, setResumeBusy] = useState<string | null>(null);
  const [renameAgent, setRenameAgent] = useState<{ id: string; name: string } | null>(null);
  const [deleteAgent, setDeleteAgent] = useState<{ id: string; confirm: string } | null>(null);
  const [picker, setPicker] = useState<{ open: boolean; mode: PickerMode; query: string }>({
    open: false,
    mode: "copy",
    query: "",
  });
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);

  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const planSteps: any[] = plan?.update?.plan ?? [];
  const running = statusLabel(statuses[activeAgent]);

  useEffect(() => {
    transcriptRef.current?.scrollTo({ top: transcriptRef.current.scrollHeight });
  }, [messages.length, streaming?.text]);

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
    setPicker({ open: true, mode: "copy", query: "" });
  };

  const openFork = () => {
    if (!activeSessionId) {
      showToast("Fork requires an active session");
      return;
    }
    setPicker({ open: true, mode: "fork", query: "" });
  };

  const selectPickerItem = async (item: PickerItem) => {
    if (picker.mode === "copy") {
      setPicker((current) => ({ ...current, open: false }));
      showToast(await copyText(item.text));
    } else {
      await props.onForkMessage(item);
      setPicker((current) => ({ ...current, open: false }));
      showToast("Creating branch; it activates when ready");
    }
  };

  const pickerItems = useMemo(() => {
    if (picker.mode === "fork") {
      return [...rawMessages]
        .reverse()
        .map((message) => ({
          id: message.id,
          role: message.role as "user" | "assistant",
          text: (message.content ?? [])
            .filter((block) => block.type === "text" && block.text)
            .map((block) => block.text ?? "")
            .join("\n")
            .trim() || `${message.role} message`,
        }));
    }
    return [...messages]
      .filter((message) => (message.role === "user" || message.role === "assistant") && message.text.trim())
      .reverse()
      .map((message) => ({ id: String(message.id), role: message.role as "user" | "assistant", text: message.text }));
  }, [messages, picker.mode, rawMessages]);

  const filteredPickerItems = useMemo(() => {
    const needle = picker.query.trim().toLowerCase();
    return pickerItems.filter((item) => !needle || `${item.role} ${item.text}`.toLowerCase().includes(needle));
  }, [picker.query, pickerItems]);

  const commands = useMemo<Command[]>(() => [
    { id: "new-agent", title: "New agent", category: "agent", keywords: "spawn create start profile", run: () => setSpawnOpen(true) },
    { id: "resume-agent", title: "Resume agent", category: "agent", keywords: "restore reconnect continue previous", run: openResume },
    { id: "new-session", title: "New session", category: "agent", keywords: "new conversation branch", run: () => setSessionCreating(true) },
    { id: "history", title: "Search prompt history", category: "agent", keywords: "ctrl r recall previous prompt", run: () => setHistoryOpen(true) },
    { id: "cancel", title: "Cancel agent", category: "agent", keywords: "stop abort interrupt", run: props.onCancel },
    { id: "compact", title: "Compact conversation", category: "agent", keywords: "compress summarize context", run: props.onCompact },
    { id: "copy", title: "Copy message", category: "clipboard", keywords: "clipboard transcript text", run: openCopy },
    { id: "fork", title: "Fork from message", category: "session", keywords: "branch conversation history", run: openFork },
    { id: "model", title: "Select model", category: "config", keywords: "model provider api key activate", run: () => props.onView("config" as View) },
    { id: "bibliography", title: "Bibliography library", category: "research", keywords: "papers articles fulltext collections", run: () => props.onView("bib" as View) },
    { id: "kms", title: "Evolution dashboard", category: "kms", keywords: "skills wiki evidence governance proposals", run: () => props.onView("kms" as View) },
    { id: "dag", title: "View DAG", category: "data", keywords: "graph pipeline topology node edge", run: () => props.onView("dag" as View) },
    { id: "thinking", title: "Toggle thinking blocks", category: "view", keywords: "collapse fold reasoning", run: () => props.onToggleDisplay("collapse_thinking") },
    { id: "tool-calls", title: "Toggle tool calls", category: "view", keywords: "collapse fold function call", run: () => props.onToggleDisplay("collapse_tool_calls") },
    { id: "tool-results", title: "Toggle tool results", category: "view", keywords: "collapse fold output", run: () => props.onToggleDisplay("collapse_tool_results") },
  ], [props, activeAgent, messages, activeSessionId]);

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

  let hiddenThinking = 0;
  let hiddenCalls = 0;
  let hiddenResults = 0;
  const visibleMessages = messages.filter((message) => {
    if (message.role === "thinking" && display.collapse_thinking) {
      hiddenThinking += 1;
      return false;
    }
    if (message.role === "tool" && message.kind === "tool_call" && display.collapse_tool_calls) {
      hiddenCalls += 1;
      return false;
    }
    if (message.role === "tool" && message.kind === "tool_result" && display.collapse_tool_results) {
      hiddenResults += 1;
      return false;
    }
    return true;
  });
  const collapsedSummary = [
    hiddenThinking && `${hiddenThinking} thinking collapsed`,
    hiddenCalls && `${hiddenCalls} tool calls collapsed`,
    hiddenResults && `${hiddenResults} results collapsed`,
  ].filter(Boolean).join(" · ");

  return (
    <section className="grid min-h-0 grid-cols-[300px_minmax(0,1fr)]">
      <ScrollArea className="min-h-0 border-r bg-muted" contentClassName="p-3">
        <div className="flex items-center justify-between gap-2">
          <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Agents</h2>
          <div className="flex gap-1">
            <Button size="xs" variant="secondary" onClick={openResume}>Resume</Button>
            <Button size="xs" onClick={() => setSpawnOpen(true)}>New</Button>
          </div>
        </div>
        <div className="mt-3 space-y-1.5">
          {agents.map((agent) => (
            <div key={agent.path} className="rounded-lg border bg-card p-1.5">
              <button
                type="button"
                onClick={() => props.onSelectAgent(agent.path)}
                className={cn("w-full rounded-md px-2 py-1.5 text-left", activeAgent === agent.path && "bg-accent")}
              >
                <span className="block truncate text-sm">{agent.path}</span>
                <span className="text-xs text-muted-foreground">{statuses[agent.path] ?? "idle"}</span>
              </button>
              <div className="mt-1 grid grid-cols-2 gap-1">
                <Button size="xs" variant="ghost" onClick={() => props.onShutdown(agent.path)}>Shutdown</Button>
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={!agent.agent_id}
                  onClick={() => props.onDelete(agent.path, agent.agent_id)}
                >
                  Delete
                </Button>
              </div>
            </div>
          ))}
        </div>

        <Separator className="my-4" />
        <div className="flex items-center justify-between">
          <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Sessions</h2>
          <Button size="xs" onClick={() => setSessionCreating(true)} disabled={!activeAgent}>New</Button>
        </div>
        {sessionCreating && (
          <form
            className="mt-2 flex gap-1.5"
            onSubmit={async (event) => {
              event.preventDefault();
              await props.onCreateSession(sessionTitle);
              setSessionTitle("");
              setSessionCreating(false);
            }}
          >
            <Input value={sessionTitle} placeholder="New session title" onChange={(event) => setSessionTitle(event.target.value)} />
            <Button type="submit" size="icon" aria-label="Create session">+</Button>
          </form>
        )}
        <div className="mt-3 space-y-1.5">
          {sessions.map((session) => (
            <div key={session.id} className="rounded-lg border p-2">
              {sessionEditor?.id === session.id ? (
                <form
                  className="flex gap-1.5"
                  onSubmit={async (event) => {
                    event.preventDefault();
                    await props.onRenameSession(session.id, sessionEditor.title);
                    setSessionEditor(null);
                  }}
                >
                  <Input value={sessionEditor.title} onChange={(event) => setSessionEditor({ id: session.id, title: event.target.value })} />
                  <Button type="submit" size="icon" aria-label="Save title">✓</Button>
                </form>
              ) : (
                <>
                  <button
                    type="button"
                    onClick={() => void props.onActivateSession(session.id)}
                    className={cn("w-full rounded-md p-1.5 text-left", activeSessionId === session.id && "bg-accent")}
                  >
                    <span className="line-clamp-2 block text-sm">{session.title || session.id}</span>
                    <span className="text-xs text-muted-foreground">{new Date(session.last_active).toLocaleString()}</span>
                  </button>
                  <div className="mt-1 grid grid-cols-2 gap-1">
                    <Button size="xs" variant="ghost" onClick={() => setSessionEditor({ id: session.id, title: session.title || "" })}>Rename</Button>
                    <Button size="xs" variant="ghost" onClick={() => void props.onCloseSession(session.id)}>Close</Button>
                  </div>
                </>
              )}
            </div>
          ))}
        </div>
      </ScrollArea>

      <div className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)_auto]">
        <div className="flex items-center justify-between gap-3 border-b px-4 py-2.5">
          <div className="min-w-0">
            <div className="truncate text-sm font-medium">{activeAgent || "No agent"}</div>
            <div className="truncate text-xs text-muted-foreground">
              {activeInfo?.model ?? "no model"} · {running}
              {usage ? ` · ${usage.input} in / ${usage.output} out` : ""}
            </div>
          </div>
          <div className="flex flex-wrap justify-end gap-1.5">
            <Button size="sm" variant="secondary" onClick={() => setPaletteOpen(true)}>Commands</Button>
            <Button size="sm" variant="secondary" onClick={openCopy}>Copy</Button>
            <Button size="sm" variant="secondary" onClick={openFork}>Fork</Button>
            <Button size="sm" variant="secondary" onClick={props.onCancel}>Cancel</Button>
            <Button size="sm" variant="secondary" onClick={props.onCompact}>Compact</Button>
          </div>
        </div>

        {planSteps.length > 0 && (
          <div className="truncate border-b px-4 py-1.5 text-xs text-muted-foreground">
            {planSteps.map((step) => `${step.status === "Completed" ? "✓" : "•"} ${step.step}`).join("    ")}
          </div>
        )}

        <ScrollArea viewportRef={transcriptRef} className="min-h-0" contentClassName="space-y-2 p-4">
          {visibleMessages.map((message) => (
            <article
              key={message.id}
              className={cn(
                "group rounded-lg border p-3 text-sm",
                message.role === "user" && "border-primary/40 bg-card",
                message.role === "assistant" && "bg-card",
                message.role === "thinking" && "opacity-80",
                message.role === "tool" && "font-mono text-xs",
                message.role === "error" && "border-destructive/40 bg-destructive/10 text-destructive",
              )}
            >
              <div className="mb-1 flex items-center gap-2 text-xs text-muted-foreground uppercase">
                <span>{message.role}</span>
                {(message.role === "user" || message.role === "assistant") && (
                  <button
                    type="button"
                    className="ml-auto opacity-0 transition hover:text-foreground group-hover:opacity-100"
                    onClick={async () => showToast(await copyText(message.text))}
                  >
                    Copy
                  </button>
                )}
              </div>
              {message.role === "assistant" ? (
                <div className="text-sm" dangerouslySetInnerHTML={{ __html: renderMarkdown(message.text) }} />
              ) : (
                <pre className="font-sans whitespace-pre-wrap">{message.text}</pre>
              )}
              {message.usage && <div className="mt-1 text-xs text-muted-foreground">{message.usage}</div>}
            </article>
          ))}
          {streaming && !visibleMessages.some((message) => message.id === streaming.id) && (
            <pre className="rounded-lg border bg-card p-3 text-sm whitespace-pre-wrap">{streaming.text}</pre>
          )}
          {collapsedSummary && <div className="pt-2 text-center text-xs text-muted-foreground">{collapsedSummary}</div>}
        </ScrollArea>

        <form
          className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 border-t bg-muted p-3"
          onSubmit={(event) => {
            event.preventDefault();
            props.onSendMessage();
          }}
        >
          <Textarea value={draft} rows={3} onChange={(event) => props.onDraftChange(event.target.value)} placeholder="输入消息；Ctrl+R 搜索 prompt 历史" />
          <Button type="submit" disabled={!draft.trim() || busy === "send"}>发送</Button>
        </form>
      </div>

      <Dialog open={spawnOpen} onOpenChange={setSpawnOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New agent</DialogTitle>
            <DialogDescription>Spawn a resident agent under an existing path.</DialogDescription>
          </DialogHeader>
          <form
            className="grid gap-3"
            onSubmit={async (event) => {
              event.preventDefault();
              await props.onSpawnAgent({ name: spawnName, parentPath: spawnParent, profile: spawnProfile });
              setSpawnOpen(false);
            }}
          >
            <Input value={spawnName} placeholder="agent name" onChange={(event) => setSpawnName(event.target.value)} required />
            <Input value={spawnParent} placeholder="/root" onChange={(event) => setSpawnParent(event.target.value)} required />
            <Input value={spawnProfile} placeholder="profile" onChange={(event) => setSpawnProfile(event.target.value)} required />
            <Button type="submit" disabled={busy === "spawn"}>Spawn</Button>
          </form>
        </DialogContent>
      </Dialog>

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
            <DialogTitle>{picker.mode === "fork" ? "Fork from message" : "Copy message"}</DialogTitle>
            <DialogDescription>
              {picker.mode === "fork" ? "Click a backend message to create a branch." : "Click a transcript message to copy it."}
            </DialogDescription>
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
        <DialogContent className="max-w-xl">
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

function statusLabel(status?: string) {
  return (status ?? "idle").toLowerCase();
}
