import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  Bot,
  Brain,
  Check,
  Cpu,
  GitFork,
  CheckCircle2,
  ChevronDown,
  Copy,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Search,
  Terminal,
  Wrench,
  X,
  XCircle,
} from "lucide-react";
import { ContextBasket, useContextBasket } from "@/features/context/context-basket";
import { serializeContext, parseContextMessage, contextKindLabel } from "@/features/context/context-model";
import { renderMarkdown } from "@/lib/markdown";
import type { AgentInfo, ChatItem, ModelRow } from "@/types";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { cn } from "@/lib/utils";
import {
  AgentStatusIcon,
  type DisplaySettings,
  copyText,
  leafName,
  preview,
  statusLabel,
} from "./agent";
import { ToolPayloadPreview, ToolPayloadView, parseMaybeJson } from "./agent-json";

/** Where the workspace renders: docked in the main or auxiliary workbench
 *  pane, in the bottom drawer overlay, or briefly standby until the docked
 *  slot mounts. The component instance survives mode changes — only the
 *  portal target moves — so transcript state never resets. */
export type AgentWorkspaceMode = "standby" | "docked" | "drawer";

const DRAWER_COLLAPSED_KEY = "autonomics.workspace.drawer.collapsed";

/** Reasoning-effort ladder; mirrors `ReasoningEffort` in
 *  crates/agentik-types/src/reasoning.rs — every variant must appear. */
const EFFORT_OPTIONS = [
  { value: "none", label: "关闭", hint: "none" },
  { value: "minimal", label: "极简", hint: "minimal" },
  { value: "low", label: "低", hint: "low" },
  { value: "medium", label: "中", hint: "medium" },
  { value: "high", label: "高", hint: "high" },
  { value: "xhigh", label: "超高", hint: "xhigh" },
  { value: "max", label: "最大", hint: "max" },
  { value: "ultra", label: "极致", hint: "ultra" },
] as const;

function effortLabel(effort: string | null | undefined) {
  return EFFORT_OPTIONS.find((option) => option.value === effort)?.label ?? "推理";
}

/** Sliding-window transcript rendering: only the tail of the message list
 *  stays mounted; earlier messages load on demand (button or scroll-to-top). */
const DEFAULT_WINDOW = 60;
const LOAD_CHUNK = 80;
const MAX_WINDOW = 500;

function formatTime(time: number) {
  return new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function parseToolCall(text: string): { name: string; input: string; inputJson: object | undefined } {
  const match = text.match(/^([\w.:-]+):\s*([\s\S]*)$/);
  if (!match) return { name: text, input: "", inputJson: undefined };
  const input = match[2];
  return { name: match[1], input, inputJson: parseMaybeJson(input) };
}

function parseToolResult(text: string) {
  if (text.startsWith("ERR ")) return { ok: false, content: text.slice(4) };
  if (text.startsWith("OK ")) return { ok: true, content: text.slice(3) };
  return { ok: true, content: text };
}

/** Consecutive reasoning/tool steps render as one Trace list; text
 *  messages sit between Trace lists (the shadcn whole-turn shape). */
type TranscriptBlock = { message: ChatItem } | { trace: ChatItem[] };

function groupTraceSteps(messages: ChatItem[]): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = [];
  for (const message of messages) {
    const last = blocks[blocks.length - 1];
    if (message.role === "thinking" || message.role === "tool") {
      if (last && "trace" in last) last.trace.push(message);
      else blocks.push({ trace: [message] });
    } else {
      blocks.push({ message });
    }
  }
  return blocks;
}

/** Chat-shaped placeholders shown while a session's history loads; the
 *  block shapes mirror real transcript rows (user bubbles right, assistant
 *  text left, trace lines at the same edge) so the swap-in is calm. */
function TranscriptSkeleton() {
  return (
    <>
      <div className="flex justify-end" aria-hidden>
        <Skeleton className="h-9 w-2/5 rounded-2xl rounded-br-md" />
      </div>
      <div className="flex flex-col gap-1.5" aria-hidden>
        <Skeleton className="h-3 w-24" />
        <Skeleton className="h-3 w-full max-w-md" />
        <Skeleton className="h-3 w-3/4" />
      </div>
      <div className="flex items-center gap-2" aria-hidden>
        <Skeleton className="size-3.5 rounded-sm" />
        <Skeleton className="h-3 w-44" />
      </div>
      <div className="flex justify-end" aria-hidden>
        <Skeleton className="h-12 w-1/3 rounded-2xl rounded-br-md" />
      </div>
      <div className="flex flex-col gap-1.5" aria-hidden>
        <Skeleton className="h-3 w-20" />
        <Skeleton className="h-3 w-full max-w-lg" />
        <Skeleton className="h-3 w-2/3" />
      </div>
    </>
  );
}

type MessageItemProps = {
  message: ChatItem;
  streamingId: number | null;
  display: DisplaySettings;
  expanded: Record<number, boolean>;
  onToggle: (id: number, open: boolean) => void;
  onCopy: (text: string) => void;
};

function MessageItem({ message, streamingId, display, expanded, onToggle, onCopy }: MessageItemProps) {
  const isStreaming = message.id === streamingId;

  if (message.role === "user") {
    const attached = parseContextMessage(message.text);
    return (
      <div className="flex justify-end">
        <div className="max-w-[85%] rounded-2xl rounded-br-md border border-primary/25 bg-primary/10 px-3.5 py-2.5 text-sm whitespace-pre-wrap sm:max-w-[75%]">
          {attached?.text ?? message.text}
          {attached && <details className="mt-2 whitespace-normal border-t pt-2 text-xs">
            <summary className="cursor-pointer">随消息发送的引用（{attached.resources.length}）</summary>
            <div className="mt-2 flex max-h-64 flex-col gap-2 overflow-y-auto">
              {attached.resources.map((resource, index) => <details key={index}>
                <summary className="cursor-pointer">{contextKindLabel[resource.kind]} · {resource.title}</summary>
                <p className="break-all text-muted-foreground">{resource.scope} · {resource.source}</p>
                <pre className="mt-1 whitespace-pre-wrap break-words">{resource.content}</pre>
              </details>)}
            </div>
          </details>}
        </div>
      </div>
    );
  }

  if (message.role === "assistant") {
    return (
      <div className="group min-w-0">
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
    );
  }

  if (message.role === "thinking") {
    const open = expanded[message.id] ?? !display.collapse_thinking;
    return (
      <TraceCollapsible open={open} onOpenChange={(next) => onToggle(message.id, next)}>
        <TraceCollapsibleTrigger>
          <TraceLine status={isStreaming ? "running" : undefined}>
            <TraceIcon>
              <Brain className="size-3.5" />
            </TraceIcon>
            <TraceTitle>{isStreaming ? "Thinking…" : "Thought process"}</TraceTitle>
            <TraceDescription>{formatTime(message.time)}</TraceDescription>
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent className="max-h-64 overflow-y-auto text-xs">
          <div className="whitespace-pre-wrap">{message.text}</div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    );
  }

  if (message.role === "tool") {
    if (message.kind === "tool_call") {
      const { name, input, inputJson } = parseToolCall(message.text);
      const open = expanded[message.id] ?? !display.collapse_tool_calls;
      return (
        <TraceCollapsible open={open} onOpenChange={(next) => onToggle(message.id, next)}>
          <TraceCollapsibleTrigger>
            <TraceLine status={isStreaming ? "running" : undefined}>
              <TraceIcon>
                <Wrench className="size-3.5" />
              </TraceIcon>
              <TraceTitle>{name}</TraceTitle>
              {input && (
                <TraceDescription className="min-w-0 shrink max-w-[min(32rem,55vw)] truncate text-xs">
                  {inputJson ? <ToolPayloadPreview value={inputJson} /> : preview(input)}
                </TraceDescription>
              )}
            </TraceLine>
          </TraceCollapsibleTrigger>
          {input && (
            <TraceCollapsibleContent className="max-h-48 overflow-y-auto font-mono text-[11px]">
              <ToolPayloadView value={inputJson} fallback={input} />
            </TraceCollapsibleContent>
          )}
        </TraceCollapsible>
      );
    }
    if (message.kind === "tool_result") {
      const { ok, content } = parseToolResult(message.text);
      const resultJson = parseMaybeJson(content);
      const open = expanded[message.id] ?? !display.collapse_tool_results;
      return (
        <TraceCollapsible open={open} onOpenChange={(next) => onToggle(message.id, next)}>
          <TraceCollapsibleTrigger>
            <TraceLine status={ok ? undefined : "error"}>
              <TraceIcon>
                {ok ? <CheckCircle2 className="text-emerald-500" /> : <XCircle />}
              </TraceIcon>
              <TraceTitle>{ok ? "Result" : "Failed"}</TraceTitle>
              {content && (
                <TraceDescription className="min-w-0 shrink max-w-[min(32rem,55vw)] truncate text-xs">
                  {resultJson ? <ToolPayloadPreview value={resultJson} /> : preview(content)}
                </TraceDescription>
              )}
            </TraceLine>
          </TraceCollapsibleTrigger>
          {content && (
            <TraceCollapsibleContent className="max-h-48 overflow-y-auto font-mono text-[11px]">
              {resultJson !== undefined ? (
                <ToolPayloadView value={resultJson} fallback={content} />
              ) : (
                <div className={cn("whitespace-pre-wrap", !ok && "text-destructive")}>{content}</div>
              )}
            </TraceCollapsibleContent>
          )}
        </TraceCollapsible>
      );
    }
    return (
      <TraceLine>
        <TraceIcon>
          <Terminal />
        </TraceIcon>
        <TraceTitle>{message.text}</TraceTitle>
      </TraceLine>
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

export type AgentWorkspaceProps = {
  mode: AgentWorkspaceMode;
  /** The center-column DOM node registered by AgentView; null while it
   *  is not mounted or has not attached yet. */
  dockedSlot: HTMLDivElement | null;
  agents: AgentInfo[];
  activeAgent: string;
  statuses: Record<string, string>;
  messages: ChatItem[];
  /** True while the active agent's session history is being fetched;
   *  renders chat-shaped skeleton placeholders instead of the transcript. */
  historyLoading: boolean;
  streaming: ChatItem | null;
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  draft: string;
  busy: string | null;
  rightOpen: boolean;
  /** Cached catalog rows (all providers) for the composer model switcher. */
  modelCatalog: ModelRow[];
  /** Fetches the catalog once; the switcher awaits it before opening so
   *  the list never shows a stale empty state. */
  onLoadModelCatalog: () => Promise<void>;
  /** Hot-swaps the agent's model to a `provider:model` spec. */
  onChangeModel: (path: string, spec: string) => Promise<void>;
  /** Effective reasoning effort for the active agent (`undefined` = not
   *  loaded yet, `null` = the model has no thinking support). */
  agentEffort?: string | null;
  /** Re-reads the effective effort; called when the active agent changes
   *  and after a model switch. */
  onRefreshEffort: () => void;
  /** Sets the per-agent reasoning-effort override. */
  onChangeEffort: (path: string, effort: string) => Promise<void>;
  onDraftChange: (value: string) => void;
  onSendMessage: (context?: string) => void;
  showInspectorToggle?: boolean;
  onSelectAgent: (path: string) => void;
  onShutdown: (path: string) => void;
  onSpawn: () => void;
  onToggleRight: () => void;
  onOpenDag: () => void;
};

export function AgentWorkspace(props: AgentWorkspaceProps) {
  const basket = useContextBasket();
  const sendMessage = () => props.onSendMessage(serializeContext(basket?.resources ?? []));
  const {
    mode,
    dockedSlot,
    agents,
    activeAgent,
    statuses,
    messages,
    historyLoading,
    streaming,
    usage,
    display,
    draft,
    busy,
    rightOpen,
    modelCatalog,
  } = props;

  const [collapsed, setCollapsed] = useState(() => {
    try { return localStorage.getItem(DRAWER_COLLAPSED_KEY) === "true"; } catch { return false; }
  });
  useEffect(() => {
    if (basket?.revealVersion) setCollapsed(false);
  }, [basket?.revealVersion]);
  // Drawer overlay body; null while the drawer is collapsed or not the
  // active hosting mode.
  const [drawerBody, setDrawerBody] = useState<HTMLDivElement | null>(null);
  // Off-document container that keeps the workspace subtree mounted while
  // no real portal target exists (first frames, hidden collapsed state).
  const detachedRef = useRef<HTMLDivElement | null>(null);
  if (!detachedRef.current && typeof document !== "undefined") {
    detachedRef.current = document.createElement("div");
  }

  useEffect(() => {
    try { localStorage.setItem(DRAWER_COLLAPSED_KEY, String(collapsed)); } catch { /* preference storage may be unavailable */ }
  }, [collapsed]);

  const target = mode === "docked" && dockedSlot
    ? dockedSlot
    : mode === "drawer" && !collapsed && drawerBody
      ? drawerBody
      : detachedRef.current!;

  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [agentPickerOpen, setAgentPickerOpen] = useState(false);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const [effortPickerOpen, setEffortPickerOpen] = useState(false);
  // Set when the catalog fetch fails; the open list shows the error
  // instead of an empty-state that would read as "no models exist".
  const [modelPickerError, setModelPickerError] = useState<string | null>(null);
  const [atBottom, setAtBottom] = useState(true);
  const [windowStart, setWindowStart] = useState(0);
  const transcriptRef = useRef<HTMLDivElement>(null);
  // The transcript subtree remounts whenever its portal target changes
  // (standby→docked slot, float collapse/expand), replacing the viewport
  // element. A plain ref would leave the scroll listener on the dead
  // element, so the mounted node doubles as reactive state to re-key the
  // listener effect below.
  const [transcriptNode, setTranscriptNode] = useState<HTMLDivElement | null>(null);
  const setTranscriptViewport = useCallback((node: HTMLDivElement | null) => {
    transcriptRef.current = node;
    setTranscriptNode(node);
  }, []);
  const atBottomRef = useRef(true);
  const windowStartRef = useRef(0);
  const pendingAnchorRef = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const docked = mode === "docked";
  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const running = statusLabel(statuses[activeAgent]);
  const currentModelSpec = activeInfo?.model ?? null;
  const currentModelShort = currentModelSpec
    ? (currentModelSpec.split(":").filter(Boolean).pop() ?? currentModelSpec)
    : null;

  const toggleModelPicker = async () => {
    if (modelPickerOpen) {
      setModelPickerOpen(false);
      return;
    }
    setEffortPickerOpen(false);
    setModelPickerError(null);
    try {
      await props.onLoadModelCatalog();
    } catch (cause) {
      setModelPickerError(String(cause));
    }
    setModelPickerOpen(true);
  };

  const toggleEffortPicker = () => {
    if (effortPickerOpen) {
      setEffortPickerOpen(false);
      return;
    }
    setModelPickerOpen(false);
    setEffortPickerOpen(true);
  };

  // Re-read the effective effort whenever the active agent changes; the
  // model switch refreshes its own path.
  useEffect(() => {
    if (!activeAgent) return;
    props.onRefreshEffort();
  }, [activeAgent, props.onRefreshEffort]);

  // Click-outside closes whichever composer popover is open. pointerdown
  // (capture) so the close lands before any click the same gesture
  // triggers elsewhere — e.g. the send button submits while the list is
  // already folding away. Both pills stay excluded: their own clicks go
  // through the toggles.
  useEffect(() => {
    if (!modelPickerOpen && !effortPickerOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Element)) return;
      if (event.target.closest("[data-composer-popover]")) return;
      setModelPickerOpen(false);
      setEffortPickerOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [modelPickerOpen, effortPickerOpen]);

  const toggleExpand = useCallback((id: number, open: boolean) => {
    setExpanded((current) => ({ ...current, [id]: open }));
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
    const viewport = transcriptNode;
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
  }, [transcriptNode, loadEarlier]);

  // A fresh viewport always starts at the top of its content; re-pin it to
  // the latest message so the remount (portal target swap, float expand)
  // does not strand the user above a stale scroll offset.
  useLayoutEffect(() => {
    if (!transcriptNode) return;
    transcriptNode.scrollTop = transcriptNode.scrollHeight;
    atBottomRef.current = true;
    setAtBottom(true);
  }, [transcriptNode]);

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

  const clampedStart = Math.min(windowStart, Math.max(0, messages.length - 1));
  const visibleMessages = messages.slice(clampedStart, atBottom ? undefined : clampedStart + MAX_WINDOW);
  const hiddenCount = clampedStart;

  const searchToggle = (
    <Button
      size="icon-sm"
      variant={agentPickerOpen ? "default" : "ghost"}
      aria-label="Search agents"
      aria-expanded={agentPickerOpen}
      title={agentPickerOpen ? "关闭 agent 搜索" : "搜索 agent"}
      onClick={() => setAgentPickerOpen((open) => !open)}
    >
      <Search />
    </Button>
  );

  const content = (
    <div
      className={cn(
        "flex min-h-0 flex-1 flex-col",
        // Entrance for (re)docking into the agent view's center column.
        mode === "docked" &&
        "animate-in fade-in-0 slide-in-from-bottom-2 duration-200 motion-reduce:animate-none",
      )}
    >
      <div className="shrink-0 border-b bg-muted/40">
        {agentPickerOpen ? (
          <Command
            className="rounded-none! bg-transparent p-0"
            loop
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setAgentPickerOpen(false);
              }
            }}
          >
            <div className="flex h-10 items-center gap-1 px-1.5">
              {searchToggle}
              <div className="min-w-0 flex-1 [&>div]:p-0">
                <CommandInput autoFocus placeholder="输入以切换 agent，Esc 返回" />
              </div>
              {docked && props.showInspectorToggle !== false && (
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label="Toggle right sidebar"
                  onClick={props.onToggleRight}
                >
                  {rightOpen ? <PanelRightClose /> : <PanelRightOpen />}
                </Button>
              )}
            </div>
            <CommandList className="max-h-64 rounded-none! border-t">
              <CommandEmpty>没有匹配的 agent</CommandEmpty>
              <CommandGroup>
                {agents.map((agent) => (
                  <CommandItem
                    key={agent.path}
                    value={`${leafName(agent.path)} ${agent.path}`}
                    data-checked={agent.path === activeAgent ? "true" : undefined}
                    onSelect={() => {
                      props.onSelectAgent(agent.path);
                      setAgentPickerOpen(false);
                    }}
                  >
                    <AgentStatusIcon status={statuses[agent.path]} />
                    <span className="truncate">{leafName(agent.path)}</span>
                    <span title={agent.path} className="min-w-0 truncate pl-2 text-xs text-muted-foreground">
                      {agent.path}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        ) : (
          <div className="flex h-10 items-center gap-1 px-1.5">
            {searchToggle}
            {/* Pinned left of the tab strip: stays visible and clickable no
                matter how far the agent tabs have scrolled. */}
            <Button size="icon-sm" variant="ghost" aria-label="New agent" className="shrink-0" onClick={props.onSpawn}>
              <Plus />
            </Button>
            {/* Vertical margins (not an explicit height): the base
                component's self-stretch needs height:auto to work — an
                explicit h-* would make it fall back to flex-start and pin
                the line to the top edge. 40px row - 2×6px = 28px, matching
                the size-7 icon buttons. */}
            <Separator orientation="vertical" className="my-1.5 shrink-0" />
            <div data-tab-strip="" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              {agents.map((agent) => (
                <div
                  key={agent.path}
                  className={cn(
                    "group/tab flex shrink-0 items-center gap-1.5 rounded-md border border-transparent py-1 pr-1 pl-2 text-xs",
                    activeAgent === agent.path ? "border-border bg-card shadow-sm" : "text-muted-foreground hover:bg-accent",
                  )}
                >
                  <AgentStatusIcon status={statuses[agent.path]} />
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
            </div>
            <Button size="icon-sm" variant="ghost" aria-label="View agent DAG" title="View agent DAG" disabled={!activeAgent} onClick={props.onOpenDag}><GitFork /></Button>
            {docked && props.showInspectorToggle !== false && (
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label="Toggle right sidebar"
                onClick={props.onToggleRight}
              >
                {rightOpen ? <PanelRightClose /> : <PanelRightOpen />}
              </Button>
            )}
          </div>
        )}
      </div>

      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs text-muted-foreground">
        <span className="truncate">{activeAgent || "No agent open"}</span>
        {activeAgent && (
          <>
            <span>·</span>
            <span className="shrink-0">{activeInfo?.model ?? "no model"}</span>
            <span>·</span>
            <AgentStatusIcon status={statuses[activeAgent]} />
            <span className="shrink-0">{running}</span>
            {usage && (
              <span className="ml-auto hidden shrink-0 sm:inline">
                {usage.input} in / {usage.output} out{usage.cache ? ` / ${usage.cache} cache` : ""}
              </span>
            )}
          </>
        )}
      </div>

      {activeAgent ? (
        <>
          <div className="relative flex min-h-0 flex-1">
            <ScrollArea viewportRef={setTranscriptViewport} className="min-h-0 flex-1" contentClassName="space-y-3 p-3 sm:p-4">
              {historyLoading ? (
                <TranscriptSkeleton />
              ) : (
                <>
                  {hiddenCount > 0 && (
                    <div className="flex justify-center pb-2">
                      <Button size="xs" variant="outline" onClick={loadEarlier}>
                        <ArrowUp />
                        Load earlier messages ({hiddenCount})
                      </Button>
                    </div>
                  )}
                  {groupTraceSteps(visibleMessages).map((block) => {
                    const renderItem = (message: ChatItem) => (
                      <MessageItem
                        key={message.id}
                        message={message}
                        streamingId={streaming?.id ?? null}
                        display={display}
                        expanded={expanded}
                        onToggle={toggleExpand}
                        onCopy={async (text) => void copyText(text)}
                      />
                    );
                    return "trace" in block ? (
                      // Trace lists start at the same edge as assistant
                      // text — the avatar indent they used to clear is gone.
                      <Trace key={block.trace[0].id}>
                        {block.trace.map(renderItem)}
                      </Trace>
                    ) : (
                      renderItem(block.message)
                    );
                  })}
                  {!messages.length && !streaming && (
                    <div className="p-8 text-center text-sm text-muted-foreground">This session has no messages yet</div>
                  )}
                </>
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

          <ContextBasket />
          <form
            className="relative shrink-0 border-t bg-muted p-2 sm:p-3"
            onSubmit={(event) => {
              event.preventDefault();
              sendMessage();
            }}
          >
            {/* Model switcher: floats above the composer card so the
                textarea keeps focus-relevant layout; the list reuses the
                same Command shape as the agent picker. */}
            {modelPickerOpen && (
              <div
                data-composer-popover=""
                className="absolute inset-x-2 bottom-full z-30 mb-2 overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-lg sm:inset-x-3"
              >
                <Command
                  loop
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.stopPropagation();
                      setModelPickerOpen(false);
                    }
                  }}
                >
                  <div className="flex h-9 items-center gap-1.5 border-b px-1.5">
                    <Cpu className="size-3.5 shrink-0 text-muted-foreground" />
                    <div className="min-w-0 flex-1 [&>div]:p-0">
                      <CommandInput autoFocus placeholder="搜索模型，Enter 切换，Esc 关闭" />
                    </div>
                  </div>
                  <CommandList className="max-h-64">
                    {modelPickerError ? (
                      <div className="p-4 text-xs text-destructive">模型目录加载失败: {modelPickerError}</div>
                    ) : (
                      <>
                        {!modelCatalog.length && (
                          <div className="p-4 text-center text-sm text-muted-foreground">
                            模型目录为空，请先在「设置 → 模型」中添加
                          </div>
                        )}
                        <CommandGroup>
                          {modelCatalog.map((model) => {
                            const spec = `${model.provider_name}:${model.model_name}`;
                            const active = spec === currentModelSpec;
                            return (
                              <CommandItem
                                key={spec}
                                value={`${model.model_name} ${model.provider_name}`}
                                data-checked={active ? "true" : undefined}
                                onSelect={() => {
                                  setModelPickerOpen(false);
                                  if (!active) void props.onChangeModel(activeAgent, spec);
                                }}
                              >
                                <span className="min-w-0 flex-1 truncate">{model.model_name}</span>
                                <span className="shrink-0 pl-2 text-xs text-muted-foreground">
                                  {model.provider_name}
                                </span>
                              </CommandItem>
                            );
                          })}
                        </CommandGroup>
                      </>
                    )}
                  </CommandList>
                </Command>
              </div>
            )}
            {/* Reasoning-effort switcher: fixed eight-step ladder, no
                search; hidden behind a disabled pill when the model lacks
                thinking support. */}
            {effortPickerOpen && (
              <div
                data-composer-popover=""
                className="absolute bottom-full left-2 z-30 mb-2 w-48 overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-lg sm:left-3"
              >
                {props.agentEffort === null ? (
                  <div className="p-3 text-xs text-muted-foreground">当前模型不支持思考</div>
                ) : (
                  <div className="p-1">
                    {EFFORT_OPTIONS.map((option) => {
                      const active = props.agentEffort === option.value;
                      return (
                        <button
                          key={option.value}
                          type="button"
                          onClick={() => {
                            setEffortPickerOpen(false);
                            if (!active) void props.onChangeEffort(activeAgent, option.value);
                          }}
                          data-checked={active ? "true" : undefined}
                          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none hover:bg-accent focus-visible:bg-accent"
                        >
                          <span>{option.label}</span>
                          <span className="text-xs text-muted-foreground">{option.hint}</span>
                          {active && <Check className="ml-auto size-3.5 shrink-0" />}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
            {/* Card-style composer: borderless textarea with a floating
                round send button, meta row below (model / hints / streaming). */}
            <div className="rounded-xl border border-input bg-background transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
              <div className="flex items-end gap-2 px-2.5 pt-2">
                <Textarea
                  value={draft}
                  rows={1}
                  onChange={(event) => props.onDraftChange(event.target.value)}
                  onKeyDown={(event) => {
                    if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && draft.trim() && busy !== "send" && busy !== "session") {
                      event.preventDefault();
                      sendMessage();
                    }
                  }}
                  placeholder="输入消息…"
                  className="max-h-48 min-h-9 resize-none overflow-y-auto border-0 bg-transparent px-0 py-1 focus-visible:ring-0 md:text-sm dark:bg-transparent"
                />
                <Button
                  type="submit"
                  size="icon"
                  className="mb-0.5 shrink-0 rounded-full"
                  disabled={!draft.trim() || busy === "send" || busy === "session"}
                  aria-label="发送"
                  title="发送（Ctrl+Enter）"
                >
                  <ArrowUp />
                </Button>
              </div>
              <div className="flex items-center gap-2 px-2.5 pb-2 pt-1 text-[11px] text-muted-foreground">
                <button
                  type="button"
                  data-composer-popover=""
                  onClick={() => void toggleModelPicker()}
                  aria-expanded={modelPickerOpen}
                  aria-label="切换模型"
                  title={currentModelSpec ? `当前模型 ${currentModelSpec}，点击切换` : "选择模型"}
                  className={cn(
                    "flex max-w-[45%] shrink-0 items-center gap-1 rounded-md border border-transparent px-1.5 py-0.5 transition hover:border-border hover:bg-background",
                    modelPickerOpen && "border-border bg-background",
                  )}
                >
                  <Cpu className="size-3 shrink-0" />
                  <span className="truncate">{currentModelShort ?? "选择模型"}</span>
                  <ChevronDown className={cn("size-3 shrink-0 transition-transform", modelPickerOpen && "rotate-180")} />
                </button>
                <button
                  type="button"
                  data-composer-popover=""
                  onClick={toggleEffortPicker}
                  aria-expanded={effortPickerOpen}
                  aria-label="推理力度"
                  disabled={props.agentEffort === null}
                  title={props.agentEffort === null ? "当前模型不支持思考" : `推理力度：${effortLabel(props.agentEffort)}，点击切换`}
                  className={cn(
                    "flex shrink-0 items-center gap-1 rounded-md border border-transparent px-1.5 py-0.5 transition hover:border-border hover:bg-background disabled:pointer-events-none disabled:opacity-50",
                    effortPickerOpen && "border-border bg-background",
                  )}
                >
                  <Brain className="size-3 shrink-0" />
                  <span className="truncate">{effortLabel(props.agentEffort)}</span>
                  <ChevronDown className={cn("size-3 shrink-0 transition-transform", effortPickerOpen && "rotate-180")} />
                </button>
                <span className="truncate">Ctrl+Enter 发送 · Ctrl+R 搜索 prompt 历史</span>
                {streaming && (
                  <span className="ml-auto flex shrink-0 items-center gap-1.5">
                    <span className="size-1.5 animate-pulse rounded-full bg-sky-400" />
                    streaming…
                  </span>
                )}
              </div>
            </div>
          </form>
        </>
      ) : (
        <div className="grid min-h-0 flex-1 place-items-center px-6 text-center text-sm text-muted-foreground">
          从侧栏选择一个 agent，或点击 + 新建
        </div>
      )}

    </div>
  );

  return (
    <>
      {mode === "drawer" && (collapsed ? (
        // Collapsed drawer: a floating action button that reopens the
        // workspace; the pulsing dot keeps streaming visible at a glance.
        <button
          type="button"
          aria-label="打开 Agent 副窗"
          title="打开 Agent 副窗"
          onClick={() => setCollapsed(false)}
          className="fixed right-4 bottom-4 z-40 grid size-12 place-items-center rounded-full border bg-card text-foreground shadow-2xl transition-colors hover:bg-accent animate-in fade-in-0 zoom-in-95 duration-200 motion-reduce:animate-none"
        >
          <Bot className="size-5" />
          {streaming && (
            <span className="absolute right-2.5 top-2.5 size-2.5 animate-pulse rounded-full border-2 border-card bg-sky-400" />
          )}
        </button>
      ) : (
        // Bottom drawer overlay: a full-screen sheet below md (the app's
        // single desktop breakpoint), an inset rounded card at md and up.
        // Non-modal — the view behind stays usable, the header button
        // folds it back to the FAB.
        <div
          data-drawer-shell=""
          className="fixed inset-0 z-40 flex flex-col overflow-hidden bg-card shadow-2xl motion-reduce:animate-none md:inset-x-3 md:top-auto md:bottom-3 md:h-[min(80dvh,640px)] md:rounded-xl md:border-t animate-in fade-in-0 slide-in-from-bottom-4 duration-200"
        >
          <div className="flex h-9 shrink-0 items-center gap-2 border-b bg-muted/50 px-2">
            <AgentStatusIcon status={statuses[activeAgent]} />
            <span className="min-w-0 truncate text-xs font-medium">{activeAgent || "Agent"}</span>
            <span className="shrink-0 text-[11px] text-muted-foreground">{running}</span>
            {streaming && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-sky-400" />}
            <Button
              size="icon-xs"
              variant="ghost"
              className="ml-auto"
              aria-label="收起 Agent 抽屉"
              aria-expanded={true}
              title="收起为副窗按钮"
              onClick={() => setCollapsed(true)}
            >
              <ChevronDown />
            </Button>
          </div>
          <div ref={setDrawerBody} className="flex min-h-0 flex-1 flex-col bg-background" />
        </div>
      ))}
      {createPortal(content, target)}
    </>
  );
}
