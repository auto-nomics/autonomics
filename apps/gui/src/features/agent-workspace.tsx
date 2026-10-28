import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { createPortal } from "react-dom";
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  Brain,
  CheckCircle2,
  ChevronDown,
  Copy,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Search,
  Sparkles,
  Terminal,
  Wrench,
  X,
  XCircle,
} from "lucide-react";
import { renderMarkdown } from "@/markdown";
import type { AgentInfo, ChatItem } from "@/types";
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
  type DisplaySettings,
  copyText,
  leafName,
  preview,
  statusDot,
  statusLabel,
} from "@/features/agent";
import { ToolPayloadView, parseMaybeJson } from "@/features/agent-json";

/** Where the workspace renders: docked in the agent view's center
 *  column, floating over any other view, or briefly standby until the
 *  docked slot mounts. The component instance survives mode changes —
 *  only the portal target moves — so transcript state never resets. */
export type AgentWorkspaceMode = "standby" | "docked" | "floating";

const FLOAT_COLLAPSED_KEY = "autonomics.workspace.float.collapsed";
const FLOAT_POS_KEY = "autonomics.workspace.float.pos";

type FloatPos = { x: number; y: number };

function readFloatPos(): FloatPos | null {
  try {
    const parsed = JSON.parse(localStorage.getItem(FLOAT_POS_KEY) ?? "null");
    return typeof parsed?.x === "number" && typeof parsed?.y === "number" ? parsed : null;
  } catch { return null; }
}

/** Keep the floating shell on screen; it must never be dragged out of reach. */
function clampPos(pos: FloatPos, width: number, height: number): FloatPos {
  const margin = 8;
  return {
    x: Math.min(Math.max(pos.x, margin), Math.max(margin, window.innerWidth - width - margin)),
    y: Math.min(Math.max(pos.y, margin), Math.max(margin, window.innerHeight - height - margin)),
  };
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
    return (
      <div className="flex justify-end">
        <div className="max-w-[85%] rounded-2xl rounded-br-md border border-primary/25 bg-primary/10 px-3.5 py-2.5 text-sm whitespace-pre-wrap sm:max-w-[75%]">
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
              {input && <TraceDescription className="max-w-60 truncate">{preview(input)}</TraceDescription>}
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
              {content && <TraceDescription className="max-w-60 truncate">{preview(content)}</TraceDescription>}
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
  streaming: ChatItem | null;
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  draft: string;
  busy: string | null;
  leftOpen: boolean;
  rightOpen: boolean;
  onDraftChange: (value: string) => void;
  onSendMessage: () => void;
  onSelectAgent: (path: string) => void;
  onShutdown: (path: string) => void;
  onSpawn: () => void;
  onToggleLeft: () => void;
  onToggleRight: () => void;
};

export function AgentWorkspace(props: AgentWorkspaceProps) {
  const {
    mode,
    dockedSlot,
    agents,
    activeAgent,
    statuses,
    messages,
    streaming,
    usage,
    display,
    draft,
    busy,
    leftOpen,
    rightOpen,
  } = props;

  const [collapsed, setCollapsed] = useState(() => {
    try { return localStorage.getItem(FLOAT_COLLAPSED_KEY) === "true"; } catch { return false; }
  });
  const [floatBody, setFloatBody] = useState<HTMLDivElement | null>(null);
  // Floating-shell position; null anchors the default bottom-right corner.
  const [floatPos, setFloatPos] = useState<FloatPos | null>(readFloatPos);
  const floatShellRef = useRef<HTMLDivElement | HTMLButtonElement | null>(null);
  // Callback form: a mutable union ref is not assignable to either element's
  // ref prop, and a stable identity avoids detach/attach churn per render.
  const setFloatShell = useCallback((node: HTMLDivElement | HTMLButtonElement | null) => {
    floatShellRef.current = node;
  }, []);
  const floatPosRef = useRef(floatPos);
  floatPosRef.current = floatPos;
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    baseX: number;
    baseY: number;
    latestX: number;
    latestY: number;
    moved: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);
  // Off-document container that keeps the workspace subtree mounted while
  // no real portal target exists (first frames, hidden collapsed state).
  const detachedRef = useRef<HTMLDivElement | null>(null);
  if (!detachedRef.current && typeof document !== "undefined") {
    detachedRef.current = document.createElement("div");
  }

  useEffect(() => {
    try { localStorage.setItem(FLOAT_COLLAPSED_KEY, String(collapsed)); } catch { /* preference storage may be unavailable */ }
  }, [collapsed]);

  // Keep the shell empty while it resizes: the workspace portals in only
  // after the expand transition settles, and leaves at once on collapse.
  const [bodyMounted, setBodyMounted] = useState(false);
  useEffect(() => {
    if (mode !== "floating" || collapsed) {
      setBodyMounted(false);
      return;
    }
    const timer = window.setTimeout(() => setBodyMounted(true), 220);
    return () => window.clearTimeout(timer);
  }, [mode, collapsed]);

  useEffect(() => {
    try {
      if (floatPos) localStorage.setItem(FLOAT_POS_KEY, JSON.stringify(floatPos));
      else localStorage.removeItem(FLOAT_POS_KEY);
    } catch { /* preference storage may be unavailable */ }
  }, [floatPos]);

  // Keep a restored position inside the viewport; it may predate a
  // smaller window or a different screen.
  useLayoutEffect(() => {
    if (mode !== "floating" || !floatPos) return;
    const shell = floatShellRef.current;
    if (!shell) return;
    const clamped = clampPos(floatPos, shell.offsetWidth, shell.offsetHeight);
    if (clamped.x !== floatPos.x || clamped.y !== floatPos.y) setFloatPos(clamped);
  }, [mode, collapsed, activeAgent, floatPos]);

  useEffect(() => {
    if (mode !== "floating") return;
    const onResize = () => {
      const shell = floatShellRef.current;
      const pos = floatPosRef.current;
      if (!shell || !pos) return;
      setFloatPos(clampPos(pos, shell.offsetWidth, shell.offsetHeight));
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [mode]);

  const startDrag = (event: ReactPointerEvent<HTMLElement>, excludeButtons: boolean) => {
    // Any fresh gesture invalidates a stale click suppression.
    suppressClickRef.current = false;
    if (event.pointerType === "mouse" && event.button !== 0) return;
    // Header controls (collapse) stay clickable instead of starting a drag.
    if (excludeButtons && (event.target as HTMLElement).closest("button")) return;
    const shell = floatShellRef.current;
    if (!shell) return;
    const rect = shell.getBoundingClientRect();
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      baseX: rect.left,
      baseY: rect.top,
      latestX: rect.left,
      latestY: rect.top,
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
    event.preventDefault();
  };

  const moveDrag = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    const shell = floatShellRef.current;
    if (!drag || !shell || event.pointerId !== drag.pointerId) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    if (!drag.moved && Math.hypot(dx, dy) < 3) return;
    drag.moved = true;
    const next = clampPos({ x: drag.baseX + dx, y: drag.baseY + dy }, shell.offsetWidth, shell.offsetHeight);
    // Track the pointer on the DOM directly: committing to state per move
    // would re-render the whole transcript every frame.
    shell.style.left = `${next.x}px`;
    shell.style.top = `${next.y}px`;
    drag.latestX = next.x;
    drag.latestY = next.y;
  };

  const endDrag = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(drag.pointerId)) {
      event.currentTarget.releasePointerCapture(drag.pointerId);
    }
    if (drag.moved) setFloatPos({ x: drag.latestX, y: drag.latestY });
    // A real drag swallows the trailing click so releasing the pointer,
    // e.g. over the pill, does not also trigger the button.
    suppressClickRef.current = drag.moved;
  };

  const target = mode === "docked" && dockedSlot
    ? dockedSlot
    : mode === "floating" && floatBody
      ? floatBody
      : detachedRef.current!;

  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [agentPickerOpen, setAgentPickerOpen] = useState(false);
  const [atBottom, setAtBottom] = useState(true);
  const [windowStart, setWindowStart] = useState(0);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const windowStartRef = useRef(0);
  const pendingAnchorRef = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const docked = mode === "docked";
  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const running = statusLabel(statuses[activeAgent]);

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

  const clampedStart = Math.min(windowStart, Math.max(0, messages.length - 1));
  const visibleMessages = messages.slice(clampedStart, atBottom ? undefined : clampedStart + MAX_WINDOW);
  const hiddenCount = clampedStart;

  const searchToggle = (
    <Button
      size="icon-sm"
      variant={agentPickerOpen ? "secondary" : "ghost"}
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
              {docked && (
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label="Toggle left sidebar"
                  onClick={props.onToggleLeft}
                >
                  {leftOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
                </Button>
              )}
              <div className="min-w-0 flex-1 [&>div]:p-0">
                <CommandInput autoFocus placeholder="输入以切换 agent，Esc 返回" />
              </div>
              {docked && (
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
                    <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[agent.path]))} />
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
            {docked && (
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label="Toggle left sidebar"
                onClick={props.onToggleLeft}
              >
                {leftOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
              </Button>
            )}
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
              <Button size="icon-sm" variant="ghost" aria-label="New agent" onClick={props.onSpawn}>
                <Plus />
              </Button>
            </div>
            {docked && (
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
            <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[activeAgent]))} />
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
            <ScrollArea viewportRef={transcriptRef} className="min-h-0 flex-1" contentClassName="space-y-3 p-3 sm:p-4">
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
                  <Trace key={block.trace[0].id} className="ml-8.5">
                    {block.trace.map(renderItem)}
                  </Trace>
                ) : (
                  renderItem(block.message)
                );
              })}
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
            className="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] gap-2 border-t bg-muted p-2 sm:p-3"
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
        <div className="grid min-h-0 flex-1 place-items-center px-6 text-center text-sm text-muted-foreground">
          从侧栏选择一个 agent，或点击 + 新建
        </div>
      )}

      <div className="flex h-6 shrink-0 items-center justify-between border-t bg-muted/40 px-3 text-[11px] text-muted-foreground">
        <span className="truncate">{activeAgent ? (streaming ? "streaming…" : running) : ""}</span>
        <span className="shrink-0">
          {messages.length ? (visibleMessages.length < messages.length ? `${visibleMessages.length} / ${messages.length} messages` : `${messages.length} messages`) : ""}
        </span>
      </div>
    </div>
  );

  return (
    <>
      {mode === "floating" ? (
        <div
          ref={setFloatShell}
          data-floating-shell=""
          style={floatPos ? { left: floatPos.x, top: floatPos.y } : { right: 16, bottom: 16 }}
          className={cn(
            "fixed z-40 flex origin-bottom-right flex-col overflow-hidden border bg-card shadow-2xl motion-reduce:animate-none",
            // The size swap doubles as the collapse animation: width and
            // height only, constant corner radius — left/top is excluded so
            // dragging never tweens.
            "transition-[width,height] duration-200",
            collapsed
              ? "h-9 w-52 rounded-lg"
              : "h-[min(80dvh,640px)] w-[min(94vw,560px)] rounded-lg",
            // Float up from the corner when first appearing.
            "animate-in fade-in-0 zoom-in-95 slide-in-from-bottom-4 duration-200",
          )}
        >
          <div
            title="拖拽移动 · 双击复位到右下角"
            onDoubleClick={() => setFloatPos(null)}
            onClick={(event) => {
              if (!collapsed) return;
              if ((event.target as HTMLElement).closest("button")) return;
              if (suppressClickRef.current) {
                suppressClickRef.current = false;
                return;
              }
              setCollapsed(false);
            }}
            onPointerDown={(event) => startDrag(event, true)}
            onPointerMove={moveDrag}
            onPointerUp={endDrag}
            onPointerCancel={endDrag}
            className={cn(
              "flex h-9 shrink-0 cursor-grab touch-none select-none items-center gap-2 border-b bg-muted/50 px-2 active:cursor-grabbing",
              collapsed && "border-b-transparent hover:bg-accent",
            )}
          >
            <span className={cn("size-1.5 shrink-0 rounded-full", statusDot(statuses[activeAgent]))} />
            <span className="min-w-0 truncate text-xs font-medium">
              {collapsed ? (activeAgent ? leafName(activeAgent) : "Agent") : activeAgent}
            </span>
            {!collapsed && <span className="shrink-0 text-[11px] text-muted-foreground">{running}</span>}
            {collapsed && streaming ? <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-sky-400" /> : null}
            <Button
              size="icon-xs"
              variant="ghost"
              className="ml-auto"
              aria-label={collapsed ? "展开 agent 浮窗" : "收起 agent 浮窗"}
              aria-expanded={!collapsed}
              title={collapsed ? "展开副窗" : "收起为迷你浮窗"}
              onClick={() => {
                if (suppressClickRef.current) {
                  suppressClickRef.current = false;
                  return;
                }
                setCollapsed((current) => !current);
              }}
            >
              <ChevronDown className={cn("transition-transform", collapsed && "rotate-180")} />
            </Button>
          </div>
          {!collapsed && bodyMounted && <div ref={setFloatBody} className="flex min-h-0 flex-1 flex-col bg-background" />}
        </div>
      ) : null}
      {createPortal(content, target)}
    </>
  );
}
