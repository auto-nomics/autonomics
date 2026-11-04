import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  Brain,
  CheckCircle2,
  CircleDashed,
  Copy,
  Plus,
  Search,
  Terminal,
  Wrench,
  XCircle,
} from "lucide-react";
import { ContextBasket, useContextBasket } from "@/features/context/context-basket";
import { serializeContext, parseContextMessage, contextKindLabel } from "@/features/context/context-model";
import { renderMarkdown } from "@/lib/markdown";
import type { AgentInfo, ChatItem, ModelRow, StoredAgent } from "@/types";
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
import { Skeleton } from "@/components/ui/skeleton";
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
import { AgentComposer } from "./composer";

/** The workspace renders through a portal into the active agent editor
 *  tab's slot (or an off-document container while no slot is attached).
 *  The single component instance survives slot changes — only the portal
 *  target moves, and per-agent state lives in the runtime store. */
export type AgentWorkspaceMode = "docked";

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

  if (message.role === "system") {
    const label =
      message.origin === "agent"
        ? message.originFrom
          ? `Agent · ${message.originFrom}`
          : "Agent"
        : "System";
    return (
      <div className="mx-auto max-w-[85%] rounded-lg border border-border/60 bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        <span className="mr-2 font-medium">{label}</span>
        <span className="whitespace-pre-wrap">{message.text}</span>
      </div>
    );
  }

  return <div className="text-center text-[11px] text-muted-foreground">{message.text}</div>;
}

export type AgentWorkspaceProps = {
  mode?: AgentWorkspaceMode;
  /** The active agent editor tab's slot DOM node; null while no agent tab
   *  is active (the detached fallback keeps the subtree mounted). */
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
  /** 终止当前回复(POST /agents/{id}/cancel)。回合进行中发送按钮变成
   *  终止键;取消后后端发 LifecycleChanged(Cancelled),composer 露出
   *  「已终止」状态直到下一个回合。 */
  onCancel: () => void;
  /** 压缩对话(POST /agents/{id}/compact)——composer 内的 Archive 图标。 */
  onCompact: () => void;
  /** Open view windows (paths) — mirrored into workbench agent tabs by
   *  the shell; shown here in the merged picker. */
  openTabs: string[];
  onSelectAgent: (path: string) => void;
  /** 关闭当前标签页——纯视图操作。virtual-actor 模式下实例归 daemon
   *  所有,空闲 TTL 到期后自动钝化;这里绝不调用 shutdown。 */
  onCloseAgent: (path: string) => void;
  onSpawn: () => void;
  /** Persisted identities (GET /storage/agents) backing the merged
   *  agent picker: resume targets whether or not an instance is live. */
  storedAgents: StoredAgent[];
  /** Server-side identity search (debounced by the caller's picker). */
  onSearchStoredAgents: (query: string) => void;
  /** Activates a stored identity (switch if live, respawn otherwise)
   *  and makes it the active tab. */
  onResumeAgent: (agent: StoredAgent) => void;
  onOpenDag: () => void;
  onOpenFlow: () => void;
};

export function AgentWorkspace(props: AgentWorkspaceProps) {
  const basket = useContextBasket();
  const sendMessage = () => props.onSendMessage(serializeContext(basket?.resources ?? []));
  const {
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
    modelCatalog,
  } = props;

  // Off-document container that keeps the workspace subtree mounted while
  // no agent editor tab hosts it.
  const detachedRef = useRef<HTMLDivElement | null>(null);
  if (!detachedRef.current && typeof document !== "undefined") {
    detachedRef.current = document.createElement("div");
  }

  const target = dockedSlot ?? detachedRef.current!;
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [agentPickerOpen, setAgentPickerOpen] = useState(false);
  // Server-side identity search: keystrokes debounce into one bounded
  // `GET /storage/agents?q=…` page instead of loading the whole table.
  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchStoredDebounced = (query: string) => {
    if (searchTimer.current) clearTimeout(searchTimer.current);
    searchTimer.current = setTimeout(() => props.onSearchStoredAgents(query), 250);
  };
  useEffect(() => () => { if (searchTimer.current) clearTimeout(searchTimer.current); }, []);
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
      if (!(button instanceof HTMLElement) || !target.contains(button)) return;
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
  }, [target]);

  // Picker groups over the three visibility classes: open tabs (views),
  // live-but-unopened instances, and dormant stored identities. The
  // stored page is bounded — dedupe by path and live id so an agent
  // whose record fell outside the page cannot render twice.
  const livePaths = new Set(agents.map((agent) => agent.path));
  const openSet = new Set(props.openTabs);
  const liveUnopened = agents.filter((agent) => !openSet.has(agent.path));
  const storedOnly = props.storedAgents.filter((stored) => {
    if (agents.some((agent) => agent.agent_id === stored.id)) return false;
    return !livePaths.has(stored.name);
  });

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
      onClick={() => {
        if (!agentPickerOpen) props.onSearchStoredAgents("");
        setAgentPickerOpen((open) => !open);
      }}
    >
      <Search />
    </Button>
  );

  const content = (
    <div className="flex min-h-0 flex-1 flex-col" data-agent-workspace={activeAgent}>
      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-1.5 text-xs text-muted-foreground">
        {searchToggle}
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

      {agentPickerOpen && (
        <div className="shrink-0 border-b bg-muted/40">
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
              <div className="min-w-0 flex-1 [&>div]:p-0">
                <CommandInput
                  autoFocus
                  placeholder="输入以切换 / 搜索 agent，Esc 返回"
                  onValueChange={(value) => searchStoredDebounced(value)}
                />
              </div>
            </div>
            <CommandList className="max-h-64 rounded-none! border-t">
              <CommandEmpty>没有匹配的 agent</CommandEmpty>
              <CommandGroup heading="新建">
                <CommandItem
                  key="new-agent"
                  value="新建 agent new spawn create"
                  onSelect={() => {
                    setAgentPickerOpen(false);
                    props.onSpawn();
                  }}
                >
                  <Plus className="size-3.5" />
                  <span>新建 agent…</span>
                </CommandItem>
              </CommandGroup>
              <CommandGroup heading="打开的 agent">
                {props.openTabs.map((path) => {
                  const live = agents.find((agent) => agent.path === path);
                  return (
                    <CommandItem
                      key={path}
                      value={`打开 ${leafName(path)} ${path}`}
                      data-checked={path === activeAgent ? "true" : undefined}
                      onSelect={() => {
                        props.onSelectAgent(path);
                        setAgentPickerOpen(false);
                      }}
                    >
                      <AgentStatusIcon status={live ? statuses[path] : "dormant"} />
                      <span className="truncate">{leafName(path)}</span>
                      <span title={path} className="min-w-0 truncate pl-2 text-xs text-muted-foreground">
                        {path}
                      </span>
                    </CommandItem>
                  );
                })}
              </CommandGroup>
              <CommandGroup heading="运行中（未打开）">
                {liveUnopened.map((agent) => (
                  <CommandItem
                    key={agent.path}
                    value={`打开 ${leafName(agent.path)} ${agent.path}`}
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
              <CommandGroup heading="已存储的 agent（选择即恢复）">
                {storedOnly.map((stored) => (
                  <CommandItem
                    key={stored.id}
                    value={`恢复 ${stored.name}`}
                    onSelect={() => {
                      setAgentPickerOpen(false);
                      void props.onResumeAgent(stored);
                    }}
                  >
                    <CircleDashed className="size-3 shrink-0 text-muted-foreground" />
                    <span className="truncate">{leafName(stored.name)}</span>
                    <span title={stored.name} className="min-w-0 truncate pl-2 text-xs text-muted-foreground">
                      {stored.name}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </div>
      )}

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
          <AgentComposer
            activeAgent={activeAgent}
            draft={draft}
            busy={busy}
            streamingActive={streaming !== null}
            status={statuses[activeAgent]}
            currentModelSpec={activeInfo?.model ?? null}
            modelCatalog={modelCatalog}
            onLoadModelCatalog={props.onLoadModelCatalog}
            onChangeModel={props.onChangeModel}
            agentEffort={props.agentEffort}
            onRefreshEffort={props.onRefreshEffort}
            onChangeEffort={props.onChangeEffort}
            onDraftChange={props.onDraftChange}
            onSend={sendMessage}
            onCancel={props.onCancel}
            onCompact={props.onCompact}
            onOpenDag={props.onOpenDag}
          />
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
      {createPortal(content, target)}
    </>
  );
}
