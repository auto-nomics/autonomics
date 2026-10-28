import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  Archive,
  ArrowDown,
  ArrowUp,
  Brain,
  CheckCircle2,
  ChevronDown,
  Copy,
  GitFork,
  History,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Plus,
  PowerOff,
  RefreshCw,
  RotateCcw,
  Sparkles,
  Square,
  Terminal,
  Trash2,
  Wrench,
  X,
  XCircle,
} from "lucide-react";
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
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

export type DisplaySettings = {
  collapse_thinking: boolean;
  collapse_tool_calls: boolean;
  collapse_tool_results: boolean;
};

type View = "agent" | "config" | "bib" | "dag";

/** Sliding-window transcript rendering: only the tail of the message list
 *  stays mounted; earlier messages load on demand (button or scroll-to-top). */
const DEFAULT_WINDOW = 60;
const LOAD_CHUNK = 80;
const MAX_WINDOW = 500;

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

function statusDot(status?: string) {
  const value = (status ?? "idle").toLowerCase();
  if (value.includes("error")) return "bg-destructive";
  if (value.includes("stream") || value.includes("request")) return "animate-pulse bg-sky-400";
  if (value.includes("execut")) return "bg-amber-400";
  if (value.includes("waiting") || value.includes("compact")) return "bg-violet-400";
  return "bg-muted-foreground/50";
}

function planStepIcon(status: unknown) {
  const value = String(status ?? "");
  if (value === "Completed") return <span className="text-emerald-500">✓</span>;
  if (value === "Cancelled") return <span className="text-destructive">✗</span>;
  if (value === "InProgress") return <span className="animate-pulse text-sky-400">◐</span>;
  return <span className="text-muted-foreground">•</span>;
}

function formatTime(time: number) {
  return new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function parseToolCall(text: string) {
  const match = text.match(/^([\w.:-]+):\s*([\s\S]*)$/);
  if (!match) return { name: text, input: "" };
  let input = match[2];
  try {
    input = JSON.stringify(JSON.parse(match[2]), null, 2);
  } catch {
    // keep raw text
  }
  return { name: match[1], input };
}

function parseToolResult(text: string) {
  if (text.startsWith("ERR ")) return { ok: false, content: text.slice(4) };
  if (text.startsWith("OK ")) return { ok: true, content: text.slice(3) };
  return { ok: true, content: text };
}

type MessageItemProps = {
  message: ChatItem;
  streamingId: number | null;
  display: DisplaySettings;
  expanded: Record<number, boolean>;
  onToggle: (id: number) => void;
  onCopy: (text: string) => void;
};

function MessageItem({ message, streamingId, display, expanded, onToggle, onCopy }: MessageItemProps) {
  const isStreaming = message.id === streamingId;

  if (message.role === "user") {
    return (
      <div className="flex justify-end">
        <div className="max-w-[75%] rounded-2xl rounded-br-md border border-primary/25 bg-primary/10 px-3.5 py-2.5 text-sm whitespace-pre-wrap">
          {message.text}
        </div>
      </div>
    );
  }

  if (message.role === "assistant") {
    return (
      <div className="group flex gap-2.5">
        <span className="mt-0.5 grid size-6 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary">
          <Sparkles className="size-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="mb-0.5 flex items-center gap-2 text-[11px] text-muted-foreground">
            <span className="font-medium">{isStreaming ? "Generating…" : "Assistant"}</span>
            {isStreaming && <span className="size-1.5 animate-pulse rounded-full bg-sky-400" />}
            <span>{formatTime(message.time)}</span>
            {message.usage && <span className="truncate">{message.usage}</span>}
            <button
              type="button"
              className="ml-auto shrink-0 opacity-0 transition hover:text-foreground group-hover:opacity-100"
              onClick={() => onCopy(message.text)}
            >
              <Copy className="size-3" />
            </button>
          </div>
          <div className="md" dangerouslySetInnerHTML={{ __html: renderMarkdown(message.text) }} />
          {isStreaming && <span className="mt-1 inline-block h-4 w-2 animate-pulse bg-foreground/70 align-text-bottom" />}
        </div>
      </div>
    );
  }

  if (message.role === "thinking") {
    const open = expanded[message.id] ?? !display.collapse_thinking;
    return (
      <div className="ml-8.5 overflow-hidden rounded-lg border bg-muted/30">
        <button
          type="button"
          onClick={() => onToggle(message.id)}
          className="flex w-full items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent/50"
        >
          <Brain className="size-3.5 shrink-0" />
          <span className="italic">{isStreaming ? "Thinking…" : "Thought process"}</span>
          {isStreaming && <span className="size-1.5 animate-pulse rounded-full bg-sky-400" />}
          <ChevronDown className={cn("ml-auto size-3.5 shrink-0 transition-transform", open && "rotate-180")} />
        </button>
        {open && (
          <pre className="max-h-64 overflow-y-auto border-t px-3 py-2 text-xs whitespace-pre-wrap text-muted-foreground italic">
            {message.text}
          </pre>
        )}
      </div>
    );
  }

  if (message.role === "tool") {
    if (message.kind === "tool_call") {
      const { name, input } = parseToolCall(message.text);
      const open = expanded[message.id] ?? !display.collapse_tool_calls;
      return (
        <div className="ml-8.5 overflow-hidden rounded-lg border bg-card font-mono text-xs">
          <button
            type="button"
            onClick={() => onToggle(message.id)}
            className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-accent/50"
          >
            <Wrench className="size-3 shrink-0 text-muted-foreground" />
            <span className="shrink-0 font-sans font-medium">{name}</span>
            {input && <span className="truncate text-muted-foreground">{preview(input)}</span>}
            <ChevronDown className={cn("ml-auto size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
          </button>
          {open && input && (
            <pre className="max-h-48 overflow-auto border-t bg-muted/40 p-2 text-[11px]">{input}</pre>
          )}
        </div>
      );
    }
    if (message.kind === "tool_result") {
      const { ok, content } = parseToolResult(message.text);
      const open = expanded[message.id] ?? !display.collapse_tool_results;
      return (
        <div
          className={cn(
            "ml-8.5 overflow-hidden rounded-lg border font-mono text-xs",
            ok ? "border-emerald-500/25" : "border-destructive/40",
          )}
        >
          <button
            type="button"
            onClick={() => onToggle(message.id)}
            className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-accent/50"
          >
            {ok ? (
              <CheckCircle2 className="size-3 shrink-0 text-emerald-500" />
            ) : (
              <XCircle className="size-3 shrink-0 text-destructive" />
            )}
            <span className="shrink-0 font-sans font-medium">{ok ? "Result" : "Error"}</span>
            {content && <span className="truncate text-muted-foreground">{preview(content)}</span>}
            <ChevronDown className={cn("ml-auto size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
          </button>
          {open && content && (
            <pre className={cn("max-h-48 overflow-auto border-t p-2 text-[11px]", ok ? "bg-muted/40" : "bg-destructive/10 text-destructive")}>
              {content}
            </pre>
          )}
        </div>
      );
    }
    return (
      <div className="ml-8.5 flex items-center gap-2 text-[11px] text-muted-foreground">
        <Terminal className="size-3 shrink-0" />
        <span className="truncate">{message.text}</span>
      </div>
    );
  }

  if (message.role === "error") {
    return (
      <div className="flex items-start gap-2.5 rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-destructive">
        <AlertTriangle className="mt-0.5 size-4 shrink-0" />
        <pre className="min-w-0 flex-1 font-sans text-xs whitespace-pre-wrap">{message.text}</pre>
      </div>
    );
  }

  return <div className="text-center text-[11px] text-muted-foreground">{message.text}</div>;
}

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
  const [leftOpen, setLeftOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [atBottom, setAtBottom] = useState(true);
  const [windowStart, setWindowStart] = useState(0);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const windowStartRef = useRef(0);
  const pendingAnchorRef = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const planSteps: any[] = plan?.update?.plan ?? [];
  const running = statusLabel(statuses[activeAgent]);

  const toggleExpand = useCallback((id: number) => {
    setExpanded((current) => ({ ...current, [id]: !current[id] }));
  }, []);

  const applyWindowStart = useCallback((next: number) => {
    windowStartRef.current = next;
    setWindowStart(next);
  }, []);

  const loadEarlier = useCallback(() => {
    const viewport = transcriptRef.current;
    const next = Math.max(0, windowStartRef.current - LOAD_CHUNK);
    if (next < windowStartRef.current && viewport) {
      pendingAnchorRef.current = { scrollTop: viewport.scrollTop, scrollHeight: viewport.scrollHeight };
    }
    applyWindowStart(next);
  }, [applyWindowStart]);

  const scrollToBottom = () => {
    applyWindowStart(Math.max(0, messages.length - DEFAULT_WINDOW));
    requestAnimationFrame(() => {
      transcriptRef.current?.scrollTo({ top: transcriptRef.current.scrollHeight, behavior: "smooth" });
    });
  };

  // Slide the window tail-forward while the user sits at the bottom; freeze it
  // in place when they have scrolled up so streaming cannot yank their view.
  useEffect(() => {
    const tail = Math.max(0, messages.length - DEFAULT_WINDOW);
    applyWindowStart(atBottomRef.current ? tail : Math.min(windowStartRef.current, Math.max(0, messages.length - 1)));
  }, [messages.length, applyWindowStart]);

  // Restore the scroll position after prepending earlier messages.
  useLayoutEffect(() => {
    const anchor = pendingAnchorRef.current;
    if (!anchor) return;
    pendingAnchorRef.current = null;
    const viewport = transcriptRef.current;
    if (viewport) {
      viewport.scrollTop = anchor.scrollTop + viewport.scrollHeight - anchor.scrollHeight;
    }
  }, [windowStart]);

  useEffect(() => {
    if (atBottomRef.current) {
      transcriptRef.current?.scrollTo({ top: transcriptRef.current.scrollHeight });
    }
  }, [messages.length, streaming?.text]);

  useEffect(() => {
    const viewport = transcriptRef.current;
    if (!viewport) return;
    const onScroll = () => {
      const bottom = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < 48;
      atBottomRef.current = bottom;
      setAtBottom(bottom);
      if (!bottom && viewport.scrollTop <= 32 && windowStartRef.current > 0 && !pendingAnchorRef.current) {
        loadEarlier();
      }
    };
    viewport.addEventListener("scroll", onScroll, { passive: true });
    return () => viewport.removeEventListener("scroll", onScroll);
  }, [activeAgent, loadEarlier]);

  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      const button = event.target instanceof HTMLElement ? event.target.closest("[data-copy-code]") : null;
      if (!(button instanceof HTMLElement)) return;
      const code = button.closest(".md-code")?.querySelector("pre");
      if (!code) return;
      void navigator.clipboard.writeText(code.textContent ?? "");
      button.textContent = "Copied!";
      window.setTimeout(() => {
        button.textContent = "Copy";
      }, 1400);
    };
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);

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

  const clampedStart = Math.min(windowStart, Math.max(0, messages.length - 1));
  const visibleMessages = messages.slice(clampedStart, atBottom ? undefined : clampedStart + MAX_WINDOW);
  const hiddenCount = clampedStart;

  return (
    <section className="flex min-h-0">
      <aside
        className={cn(
          "flex h-full min-h-0 shrink-0 overflow-hidden border-r bg-muted/50 transition-[width] duration-200",
          leftOpen ? "w-[300px]" : "w-0 border-transparent",
        )}
      >
        <ScrollArea className="min-h-0 w-[300px]" contentClassName="flex flex-col gap-1 p-3">
          <div className="flex items-center justify-between gap-2 pb-1">
            <h2 className="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Agents</h2>
            <div className="flex gap-0.5">
              <Button size="icon-xs" variant="ghost" aria-label="Resume agent" onClick={openResume}>
                <RotateCcw />
              </Button>
              <Button size="icon-xs" variant="ghost" aria-label="New agent" onClick={() => setSpawnOpen(true)}>
                <Plus />
              </Button>
            </div>
          </div>
          {agents.map((agent) => (
            <div
              key={agent.path}
              className={cn(
                "group/agent flex items-center gap-0.5 rounded-md px-1",
                activeAgent === agent.path && "bg-accent",
              )}
            >
              <button
                type="button"
                onClick={() => props.onSelectAgent(agent.path)}
                className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 py-1.5 text-left hover:bg-accent"
                title={agent.path}
              >
                <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[agent.path]))} />
                <span className="truncate text-sm">{agent.path}</span>
              </button>
              <span className="flex shrink-0 items-center opacity-0 transition group-hover/agent:opacity-100">
                <Button
                  size="icon-xs"
                  variant="ghost"
                  aria-label="Shutdown agent"
                  onClick={() => props.onShutdown(agent.path)}
                >
                  <PowerOff />
                </Button>
                <Button
                  size="icon-xs"
                  variant="ghost"
                  aria-label="Delete agent"
                  disabled={!agent.agent_id}
                  onClick={() => props.onDelete(agent.path, agent.agent_id)}
                >
                  <Trash2 />
                </Button>
              </span>
            </div>
          ))}
        </ScrollArea>
      </aside>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="flex h-10 shrink-0 items-center gap-1 border-b bg-muted/40 px-1.5">
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Toggle left sidebar"
            onClick={() => setLeftOpen((current) => !current)}
          >
            {leftOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
          </Button>
          <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
            {agents.map((agent) => (
              <div
                key={agent.path}
                className={cn(
                  "group/tab flex shrink-0 items-center gap-1.5 rounded-md border border-transparent py-1 pr-1 pl-2 text-xs",
                  activeAgent === agent.path ? "border-border bg-card shadow-sm" : "text-muted-foreground hover:bg-accent",
                )}
              >
                <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[agent.path]))} />
                <button
                  type="button"
                  onClick={() => props.onSelectAgent(agent.path)}
                  className="max-w-40 truncate"
                  title={agent.path}
                >
                  {leafName(agent.path)}
                </button>
                <button
                  type="button"
                  aria-label="Shutdown agent"
                  title="关闭并停止 agent"
                  onClick={() => props.onShutdown(agent.path)}
                  className="rounded-sm p-1 opacity-0 transition hover:bg-accent group-hover/tab:opacity-100"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
            <Button size="icon-sm" variant="ghost" aria-label="New agent" onClick={() => setSpawnOpen(true)}>
              <Plus />
            </Button>
          </div>
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Toggle right sidebar"
            onClick={() => setRightOpen((current) => !current)}
          >
            {rightOpen ? <PanelRightClose /> : <PanelRightOpen />}
          </Button>
        </div>

        <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs text-muted-foreground">
          <span className="truncate">{activeAgent || "No agent open"}</span>
          {activeAgent && (
            <>
              <span>·</span>
              <span className="shrink-0">{activeInfo?.model ?? "no model"}</span>
              <span>·</span>
              <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[activeAgent]))} />
              <span className="shrink-0">{running}</span>
              {usage && (
                <span className="ml-auto shrink-0">
                  {usage.input} in / {usage.output} out{usage.cache ? ` / ${usage.cache} cache` : ""}
                </span>
              )}
            </>
          )}
        </div>

        {activeAgent ? (
          <>
            <div className="relative flex min-h-0 flex-1">
              <ScrollArea viewportRef={transcriptRef} className="min-h-0 flex-1" contentClassName="space-y-3 p-4">
                {hiddenCount > 0 && (
                  <div className="flex justify-center pb-2">
                    <Button size="xs" variant="outline" onClick={loadEarlier}>
                      <ArrowUp />
                      Load earlier messages ({hiddenCount})
                    </Button>
                  </div>
                )}
                {visibleMessages.map((message) => (
                  <MessageItem
                    key={message.id}
                    message={message}
                    streamingId={streaming?.id ?? null}
                    display={display}
                    expanded={expanded}
                    onToggle={toggleExpand}
                    onCopy={async (text) => showToast(await copyText(text))}
                  />
                ))}
                {!messages.length && !streaming && (
                  <div className="p-8 text-center text-sm text-muted-foreground">This session has no messages yet</div>
                )}
              </ScrollArea>
              {!atBottom && (
                <button
                  type="button"
                  aria-label="Scroll to bottom"
                  onClick={scrollToBottom}
                  className="absolute right-4 bottom-4 grid size-8 place-items-center rounded-full border bg-popover text-muted-foreground shadow-lg hover:bg-accent hover:text-foreground"
                >
                  <ArrowDown className="size-4" />
                </button>
              )}
            </div>

            <form
              className="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] gap-2 border-t bg-muted p-3"
              onSubmit={(event) => {
                event.preventDefault();
                props.onSendMessage();
              }}
            >
              <Textarea value={draft} rows={3} onChange={(event) => props.onDraftChange(event.target.value)} placeholder="输入消息；Ctrl+R 搜索 prompt 历史" />
              <Button type="submit" disabled={!draft.trim() || busy === "send"}>发送</Button>
            </form>
          </>
        ) : (
          <div className="grid min-h-0 flex-1 place-items-center text-sm text-muted-foreground">
            从左侧选择一个 agent，或点击 + 新建
          </div>
        )}

        <div className="flex h-6 shrink-0 items-center justify-between border-t bg-muted/40 px-3 text-[11px] text-muted-foreground">
          <span className="truncate">{activeAgent ? (streaming ? "streaming…" : running) : ""}</span>
          <span className="shrink-0">
            {messages.length ? (visibleMessages.length < messages.length ? `${visibleMessages.length} / ${messages.length} messages` : `${messages.length} messages`) : ""}
          </span>
        </div>
      </div>

      <aside
        className={cn(
          "flex h-full min-h-0 shrink-0 overflow-hidden border-l bg-muted/50 transition-[width] duration-200",
          rightOpen ? "w-[264px]" : "w-0 border-transparent",
        )}
      >
        <ScrollArea className="min-h-0 w-[264px]" contentClassName="space-y-4 p-3">
          <section>
            <div className="mb-1.5 flex items-center justify-between">
              <h2 className="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Sessions</h2>
              <Button size="icon-xs" variant="ghost" aria-label="New session" onClick={() => setSessionCreating(true)} disabled={!activeAgent}>
                <Plus />
              </Button>
            </div>
            {sessionCreating && (
              <form
                className="flex gap-1.5"
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
            <div className="space-y-1">
              {sessions.map((session) => (
                <div
                  key={session.id}
                  className={cn(
                    "group/session rounded-md px-1",
                    activeSessionId === session.id && "bg-accent",
                  )}
                >
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
                    <div className="flex items-center gap-0.5">
                      <button
                        type="button"
                        onClick={() => void props.onActivateSession(session.id)}
                        className="min-w-0 flex-1 rounded-md px-1.5 py-1.5 text-left hover:bg-accent"
                        title={session.title || session.id}
                      >
                        <span className="line-clamp-2 block text-sm">{session.title || session.id}</span>
                        <span className="text-xs text-muted-foreground">{new Date(session.last_active).toLocaleString()}</span>
                      </button>
                      <span className="flex shrink-0 flex-col opacity-0 transition group-hover/session:opacity-100">
                        <Button
                          size="icon-xs"
                          variant="ghost"
                          aria-label="Rename session"
                          onClick={() => setSessionEditor({ id: session.id, title: session.title || "" })}
                        >
                          <Pencil className="size-2.5" />
                        </Button>
                        <Button size="icon-xs" variant="ghost" aria-label="Close session" onClick={() => void props.onCloseSession(session.id)}>
                          <X className="size-2.5" />
                        </Button>
                      </span>
                    </div>
                  )}
                </div>
              ))}
              {!sessions.length && (
                <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No sessions</div>
              )}
            </div>
          </section>

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
            <div className="space-y-0.5">
              {([
                ["collapse_thinking", "Collapse thinking"],
                ["collapse_tool_calls", "Collapse tool calls"],
                ["collapse_tool_results", "Collapse tool results"],
              ] as const).map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  onClick={() => props.onToggleDisplay(key)}
                  className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-sm hover:bg-accent"
                >
                  <span>{label}</span>
                  <span
                    className={cn(
                      "flex h-4 w-7 items-center rounded-full px-0.5 transition-colors",
                      display[key] ? "justify-end bg-primary" : "justify-start bg-muted-foreground/30",
                    )}
                  >
                    <span className="size-3 rounded-full bg-background" />
                  </span>
                </button>
              ))}
            </div>
          </section>

          <section>
            <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Actions</h2>
            <div className="grid grid-cols-2 gap-1.5">
              <Button size="xs" variant="secondary" onClick={props.onCancel}><Square />Cancel</Button>
              <Button size="xs" variant="secondary" onClick={props.onCompact}><Archive />Compact</Button>
              <Button size="xs" variant="secondary" onClick={openCopy}><Copy />Copy</Button>
              <Button size="xs" variant="secondary" onClick={openFork}><GitFork />Fork</Button>
              <Button size="xs" variant="secondary" onClick={() => setHistoryOpen(true)}><History />History</Button>
              <Button size="xs" variant="secondary" onClick={props.onRefreshHistory}><RefreshCw />Refresh</Button>
            </div>
          </section>
        </ScrollArea>
      </aside>

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
