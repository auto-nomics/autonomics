// Agent runtime store: gateway connection, login, the per-agent event
// reducer and every agent/context action. The shell (app-view) consumes this
// hook and renders; no JSX lives here.
import { useCallback, useEffect, useRef, useState } from "react";
import {
  GatewayHttpError,
  gatewayRequest,
  loginGateway,
  logoutGateway,
  probeGateway,
  startGatewayEvents,
} from "@/gateway";
import type {
  AgentInfo,
  BackendMessage,
  ChatItem,
  GatewayFrame,
  ModelRow,
  ContextInfo,
  StateSnapshot,
  StoredAgent,
} from "@/types";
import { formatTokenUsage } from "@/lib/token-format";
import type { DisplaySettings } from "@/features/agent";

type MessageMap = Record<string, ChatItem[]>;
type RawMessageMap = Record<string, BackendMessage[]>;
type PromptHistoryMap = Record<string, string[]>;
type PlanMap = Record<string, { revision: number; update: any }>;
type UsageMap = Record<string, { input: number; output: number; cache: number }>;

/** One row of the workbench panel's 事件流: a gateway frame summary.
 *  Text/Thinking deltas are elided (they are per-token noise; the
 *  transcript shows the same content). */
export type FrameLogEntry = { seq: number; kind: string; summary: string; time: number };

/** A gateway notice frame (model_changed, chatgpt_login, catalog_fetched)
 *  — surfaced in the panel's 通知 tab and the unread badge. */
export type NoticeEntry = { seq: number; data: unknown; time: number };

const FRAME_LOG_LIMIT = 200;
const NOTICE_LIMIT = 100;

/** Text/Thinking deltas never land in the event log. */
function isDeltaOnlyFrame(frame: GatewayFrame): boolean {
  if (frame.kind !== "agent") return false;
  const event = frame.data?.event;
  return event !== null && typeof event === "object"
    && (event.TextDelta !== undefined || event.ThinkingDelta !== undefined || event.Compact?.event?.CompactSummaryDelta !== undefined);
}

function frameSummary(frame: GatewayFrame): string {
  if (frame.kind === "agent") {
    const event = frame.data?.event ?? {};
    const key = Object.keys(event)[0] ?? "event";
    const value = event[key];
    // Transcript-echoed payloads (the user's own prompt, error text) are
    // rendered verbatim in the transcript — the event log records the
    // fact, not the content.
    const detail = value !== null && typeof value === "object"
      || key === "UserMessageAcknowledged"
      || key === "Error"
      ? ""
      : ` ${String(value).slice(0, 60)}`;
    return `${key}${detail} · ${frame.data?.agent ?? ""}`;
  }
  if (frame.kind === "host") {
    const data = frame.data ?? {};
    return `${data.kind ?? "host"} ${data.path ?? data.info?.path ?? ""}`;
  }
  if (frame.kind === "notice") {
    const data = frame.data ?? {};
    return Object.keys(data).join(",") || "notice";
  }
  return "events lag — resuming";
}

/** localStorage key for the open-tab (view-window) list. */
const OPEN_TABS_KEY = "autonomics.openTabs";

let itemSequence = 0;
function makeItem(
  role: ChatItem["role"],
  text: string,
  kind?: ChatItem["kind"],
  origin?: { tag: "system" | "agent"; from?: string },
): ChatItem {
  itemSequence += 1;
  return {
    id: itemSequence,
    role,
    text,
    kind,
    origin: origin?.tag,
    originFrom: origin?.from,
    time: Date.now(),
  };
}

/** Normalize a backend MessageOrigin (bare tag or {agent:{path}}) into the
 *  item-level shape; undefined for user-authored content. */
function normalizeOrigin(
  origin: unknown,
): { tag: "system" | "agent"; from?: string } | undefined {
  if (origin == null || origin === "user") return undefined;
  if (typeof origin === "string") {
    return origin === "agent" ? { tag: "agent" } : { tag: "system" };
  }
  const path = (origin as { agent?: { path?: unknown } }).agent?.path;
  if (typeof path === "string") return { tag: "agent", from: path };
  return { tag: "system" };
}

function convertMessage(message: any): ChatItem[] {
  const output: ChatItem[] = [];
  // Non-user origins (delegations, background notices, harness feedback)
  // ride in user-role messages on the wire; surface them as system items.
  const origin = message.role === "user" ? normalizeOrigin(message.origin) : undefined;
  const textRole: ChatItem["role"] =
    message.role === "user" ? (origin ? "system" : "user") : "assistant";
  let text = "";
  const pushText = () => {
    if (text) output.push(makeItem(textRole, text, undefined, origin));
    text = "";
  };
  for (const block of message.content ?? []) {
    if (block.type === "text") text += block.text ? `${text ? "\n" : ""}${block.text}` : "";
    else if (block.type === "thinking") output.push(makeItem("thinking", block.thinking ?? "", "thinking"));
    else if (block.type === "tool_use") output.push(makeItem("tool", `${block.name}: ${JSON.stringify(block.input)}`, "tool_call"));
    else if (block.type === "tool_result") {
      // Backend content is ToolResultContent: a string, an arbitrary JSON
      // value, or a list of text/image blocks. Text blocks join; images
      // leave a marker; other values serialize so the renderer can parse.
      const content = block.content;
      const resultText = typeof content === "string"
        ? content
        : Array.isArray(content)
          ? content.map((item: any) => (item?.type === "text" ? item.text : `[${item?.type ?? "block"}]`)).join("\n")
          : content == null
            ? ""
            : JSON.stringify(content);
      output.push(makeItem("tool", `${block.is_error ? "ERR" : "OK"} ${resultText}`, "tool_result"));
    }
  }
  pushText();
  return output;
}

function statusLabel(status?: unknown) {
  // The daemon's status is `AgentLifecycleStatus` (the unified enum, no
  // coarse host projection anymore), serialized as an UPPERCASE tag
  // string — "IDLE", "TOOLRUNNING", "RETRYING", … Host projection frames
  // may carry the tagged object form `{ kind: "running" }`.
  const raw = typeof status === "string"
    ? status
    : status !== null && typeof status === "object" && "kind" in (status as Record<string, unknown>)
      ? String((status as Record<string, unknown>).kind)
      : "idle";
  return raw.toLowerCase().replace(/^.*\./, "");
}

export function useAgentRuntime() {
  const [booting, setBooting] = useState(true);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  // The web build authenticates with a session cookie; when the daemon
  // rejects the request the user is asked for the gateway token once.
  const [needsLogin, setNeedsLogin] = useState(false);
  const [loginToken, setLoginToken] = useState("");
  const [loginError, setLoginError] = useState<string | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  // Open tabs are view windows over identities, NOT the live agent list:
  // a tab opens only by user action (spawn, resume, picker select) and
  // survives its agent's idle-TTL passivation — the tab just goes
  // dormant (identity stored, instance evicted) until a message
  // re-activates it. Persisted so a reload reopens the same views.
  const [openTabs, setOpenTabs] = useState<string[]>(() => {
    try {
      const raw = JSON.parse(localStorage.getItem(OPEN_TABS_KEY) ?? "[]");
      return Array.isArray(raw) ? raw.filter((item): item is string => typeof item === "string") : [];
    } catch { return []; }
  });
  // Persisted identities (GET /storage/agents) for the merged agent
  // picker: resume targets whether or not the instance is live.
  const [storedAgents, setStoredAgents] = useState<StoredAgent[]>([]);
  const [storedIds, setStoredIds] = useState<Record<string, string>>({});
  const [contexts, setContexts] = useState<Record<string, ContextInfo>>({});
  const [activeContexts, setActiveContexts] = useState<Record<string, string | null>>({});
  const [activeAgent, setActiveAgent] = useState("");
  const [statuses, setStatuses] = useState<Record<string, string>>({});
  const [messages, setMessages] = useState<MessageMap>({});
  const [compactions, setCompactions] = useState<Record<string, import("@/types").CompactionView>>({});
  const [historyLoading, setHistoryLoading] = useState<Record<string, boolean>>({});
  const [streaming, setStreaming] = useState<Record<string, ChatItem | null>>({});
  const [rawMessages, setRawMessages] = useState<RawMessageMap>({});
  const [promptHistories, setPromptHistories] = useState<PromptHistoryMap>({});
  const [plans, setPlans] = useState<PlanMap>({});
  const [usage, setUsage] = useState<UsageMap>({});
  const [display, setDisplay] = useState<DisplaySettings>({
    collapse_thinking: false,
    collapse_tool_calls: false,
    collapse_tool_results: false,
  });
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const draftsRef = useRef(drafts);
  draftsRef.current = drafts;
  const [sendingPaths, setSendingPaths] = useState<Record<string, boolean>>({});
  const sendingRef = useRef(new Set<string>());
  const setDraftFor = useCallback((path: string, value: string) => {
    setDrafts(current => ({ ...current, [path]: value }));
  }, []);
  const draft = drafts[activeAgent] ?? "";
  const setDraft = (value: string) => setDraftFor(activeAgent, value);
  const [busy, setBusy] = useState<string | null>(null);
  // Model catalog for the composer switcher; fetched lazily on first open.
  const [modelCatalog, setModelCatalog] = useState<ModelRow[]>([]);
  const modelCatalogLoaded = useRef(false);
  // Bottom-panel feeds: the frame log (delta frames elided) and notices.
  const [frameLog, setFrameLog] = useState<FrameLogEntry[]>([]);
  const [notices, setNotices] = useState<NoticeEntry[]>([]);

  const stateRef = useRef({ agents, contexts, activeAgent, activeContexts, storedIds });
  stateRef.current = { agents, contexts, activeAgent, activeContexts, storedIds };
  const openTabsRef = useRef(openTabs);
  openTabsRef.current = openTabs;
  const streamingRef = useRef<Record<string, ChatItem | null>>({});
  const historyLoadSeq = useRef<Record<string, number>>({});
  const loadedHistory = useRef<Record<string, string>>({});
  const historyInFlight = useRef(new Map<string, Promise<void>>());
  // Events received while a transcript is being fetched must be applied
  // after the snapshot, otherwise the response replaces the live updates.
  const historyEventBuffers = useRef<Record<string, { contextId: string; events: GatewayFrame[] }>>({});

  const appendMessage = useCallback((agent: string, item: ChatItem) => {
    setMessages((current) => ({ ...current, [agent]: [...(current[agent] ?? []), item] }));
  }, []);

  const loadHistory = useCallback(async (agentPath: string, explicitContextId?: string) => {
    const info = stateRef.current.agents.find((agent) => agent.path === agentPath);
    const contextId = explicitContextId ?? stateRef.current.activeContexts[agentPath]
      ?? stateRef.current.contexts[agentPath]?.id ?? null;
    const agentId = info?.agent_id ?? stateRef.current.storedIds[agentPath];
    if (!agentId || !contextId) return;
    // Only the latest in-flight load per agent may clear the flag, so a
    // superseded fetch (fast context double-switch) cannot hide the
    // skeleton while its successor is still running.
    const seq = (historyLoadSeq.current[agentPath] ?? 0) + 1;
    historyLoadSeq.current[agentPath] = seq;
    if (historyEventBuffers.current[agentPath]?.contextId !== contextId) {
      historyEventBuffers.current[agentPath] = { contextId, events: [] };
    }
    setHistoryLoading((current) => ({ ...current, [agentPath]: true }));
    try {
      const response = await gatewayRequest(
        "GET",
        `/agents/${agentId}/sessions/${contextId}/history`,
      );
      const history: ChatItem[] = (Array.isArray(response) ? response : response.messages ?? [])
        .flatMap(convertMessage);
      if (historyLoadSeq.current[agentPath] !== seq) return;
      loadedHistory.current[agentPath] = `${agentId}:${contextId}`;
      setMessages((current) => ({ ...current, [agentPath]: history }));
      setRawMessages((current) => ({
        ...current,
        [`${agentPath}:${contextId}`]: Array.isArray(response) ? response : response.messages ?? [],
      }));
    } catch (cause) {
      if (historyLoadSeq.current[agentPath] === seq) {
        appendMessage(agentPath, makeItem("error", `加载历史失败: ${String(cause)}`));
      }
    } finally {
      if (historyLoadSeq.current[agentPath] === seq) {
        const buffered = historyEventBuffers.current[agentPath]?.events ?? [];
        delete historyEventBuffers.current[agentPath];
        for (const frame of buffered) handleFrameRef.current(frame);
        setHistoryLoading((current) => ({ ...current, [agentPath]: false }));
      }
    }
  }, [appendMessage]);

  /** Reuse the current transcript when a tab is revisited. Explicit refresh
   *  still calls loadHistory directly and bypasses this cache. */
  const ensureAgentHistory = useCallback((agentPath: string, contextId: string) => {
    const agentId = stateRef.current.agents.find(agent => agent.path === agentPath)?.agent_id ?? stateRef.current.storedIds[agentPath];
    if (!agentId || !contextId) return;
    const version = `${agentId}:${contextId}`;
    if (loadedHistory.current[agentPath] === version) return;
    const key = JSON.stringify([agentPath, version]);
    if (historyInFlight.current.has(key)) return;
    const pending = loadHistory(agentPath, contextId).finally(() => historyInFlight.current.delete(key));
    historyInFlight.current.set(key, pending);
    return pending;
  }, [loadHistory]);

  // A resumed agent and its context are announced asynchronously. Selecting
  // the tab can happen before either ID is available, so load once both are
  // present, and again when the user selects a different agent or context.
  const selectedAgentId = agents.find((agent) => agent.path === activeAgent)?.agent_id ?? storedIds[activeAgent];
  const selectedContextId = activeContexts[activeAgent] ?? contexts[activeAgent]?.id;
  useEffect(() => {
    if (activeAgent && selectedAgentId && selectedContextId) {
      void ensureAgentHistory(activeAgent, selectedContextId);
    }
  }, [activeAgent, selectedAgentId, selectedContextId, ensureAgentHistory]);

  useEffect(() => {
    let cancelled = false;
    for (const path of openTabs) {
      if (agents.some(agent => agent.path === path && agent.agent_id) && contexts[path]) continue;
      void (async () => {
        try {
          const page: StoredAgent[] = await gatewayRequest("GET", `/storage/agents?q=${encodeURIComponent(path)}&limit=100`);
          const record = page.find(agent => agent.name === path);
          if (!record || cancelled) return;
          setStoredIds(current => current[path] === record.id ? current : { ...current, [path]: record.id });
          if (contexts[path]) return;
          const sessions: ContextInfo[] = await gatewayRequest("GET", `/storage/agents/${record.id}/sessions`);
          const latest = sessions.reduce<ContextInfo | undefined>((a, b) => !a || b.last_active > a.last_active ? b : a, undefined);
          if (latest && !cancelled) {
            setContexts(current => current[path] ? current : { ...current, [path]: latest });
            setActiveContexts(current => current[path] ? current : { ...current, [path]: latest.id });
          }
        } catch (error) { console.warn("Failed to load agent history metadata", error); }
      })();
    }
    return () => { cancelled = true; };
  }, [openTabs, agents, contexts]);

  /** Records a live frame into the bottom-panel feeds. Called only on
   *  arrival — buffered events replay straight into handleFrame so the
   *  log shows each frame exactly once. */
  const recordFrame = useCallback((frame: GatewayFrame) => {
    if (isDeltaOnlyFrame(frame)) return;
    const entry: FrameLogEntry = { seq: frame.seq, kind: frame.kind, summary: frameSummary(frame), time: Date.now() };
    setFrameLog((current) => [...current.slice(-(FRAME_LOG_LIMIT - 1)), entry]);
    if (frame.kind === "notice") {
      const noticeKind = (frame.data as { kind?: string } | null)?.kind;
      if (noticeKind === "compute_changed") {
        // 计算目录变更 → HPC 计算目标编辑器即时刷新。
        window.dispatchEvent(new CustomEvent("autonomics:compute-changed"));
      }
      setNotices((current) => [...current.slice(-(NOTICE_LIMIT - 1)), { seq: frame.seq, data: frame.data, time: Date.now() }]);
    }
  }, []);

  const handleFrame = useCallback((frame: GatewayFrame) => {
    if (frame.kind === "lag") {
      void hydrateRef.current?.();
      return;
    }
    if (frame.kind === "agent") {
      const agent = frame.data.agent;
      const event = frame.data.event;
      const buffer = historyEventBuffers.current[agent];
      const announcedContext = event.ContextActivated?.id ?? event.ContextList?.context?.id;
      if (buffer && announcedContext && announcedContext !== buffer.contextId) {
        historyLoadSeq.current[agent] += 1;
        delete historyEventBuffers.current[agent];
        setHistoryLoading((current) => ({ ...current, [agent]: false }));
      }
      // Context announcements must continue to flow: they establish the
      // identity needed for loading. Other events retain their original
      // ordering and are replayed after the snapshot (also on failure).
      if (historyEventBuffers.current[agent] && event.ContextList === undefined && event.ContextActivated === undefined) {
        historyEventBuffers.current[agent].events.push(frame);
        return;
      }
      const appendStreamDelta = (
        role: "assistant" | "thinking",
        delta: string,
      ) => {
        const current = streamingRef.current[agent];
        const item = current?.role === role
          ? current
          : makeItem(role, "");
        item.text += delta;
        streamingRef.current[agent] = item;
        setStreaming((currentMessages) => ({ ...currentMessages, [agent]: item }));
        setMessages((currentMessages) => {
          const existing = currentMessages[agent] ?? [];
          return {
            ...currentMessages,
            [agent]: existing.some((message) => message.id === item.id)
              ? existing.map((message) => message.id === item.id ? item : message)
              : [...existing, item],
          };
        });
      };
      if (event.Compact !== undefined) {
        const compact = event.Compact.event;
        setCompactions(current => {
          const previous = current[agent];
          if (compact?.CompactStart) return { ...current, [agent]: { running: true, phase: null, summary: "", plan: compact.CompactStart.plan ?? null, stats: null, error: null } };
          if (!previous) return current;
          if (compact?.CompactPhase) return { ...current, [agent]: { ...previous, phase: compact.CompactPhase.phase } };
          if (compact?.CompactSummaryDelta) return { ...current, [agent]: { ...previous, summary: previous.summary + compact.CompactSummaryDelta.text } };
          if (compact?.CompactFinish) return { ...current, [agent]: { ...previous, running: false, phase: null, stats: compact.CompactFinish.stats ?? null, error: compact.CompactFinish.error ?? null } };
          return current;
        });
      } else if (event.TextDelta !== undefined) {
        appendStreamDelta("assistant", event.TextDelta);
      } else if (event.ThinkingDelta !== undefined) {
        appendStreamDelta("thinking", event.ThinkingDelta);
      } else if (event.ToolCall !== undefined) {
        appendMessage(agent, makeItem("tool", `${event.ToolCall.name}: ${JSON.stringify(event.ToolCall.input)}`, "tool_call"));
      } else if (event.ToolResult !== undefined) {
        appendMessage(agent, makeItem("tool", `${event.ToolResult.ok ? "OK" : "ERR"} ${event.ToolResult.content}`, "tool_result"));
      } else if (event.ToolCallBackground !== undefined) {
        appendMessage(agent, makeItem("tool", `background #${event.ToolCallBackground.seq}: ${event.ToolCallBackground.name}`));
      } else if (event.ToolBackgroundComplete !== undefined) {
        appendMessage(agent, makeItem("tool", `background #${event.ToolBackgroundComplete.seq} ${event.ToolBackgroundComplete.ok ? "完成" : "失败"}`));
      } else if (event.UserMessageAcknowledged !== undefined) {
        appendMessage(agent, makeItem("user", event.UserMessageAcknowledged));
        // Only user-authored prompts belong in the Ctrl+R history;
        // MessageInjected items are non-user (delegate/background/harness).
        setPromptHistories((current) => {
          const history = current[agent] ?? [];
          return history.includes(event.UserMessageAcknowledged) ? current : { ...current, [agent]: [...history, event.UserMessageAcknowledged].slice(-200) };
        });
      } else if (event.MessageInjected !== undefined) {
        const injected = event.MessageInjected;
        appendMessage(agent, makeItem("system", injected.text, undefined, normalizeOrigin(injected.origin)));
      } else if (event.PlanUpdate !== undefined) {
        setPlans((current) => ({ ...current, [agent]: event.PlanUpdate }));
      } else if (event.RetryableError !== undefined) {
        // Transient API failure in request_with_retries: the agent backs
        // off and re-sends the same request, so this renders as progress
        // info in the transcript — the terminal Error item is separate.
        const retry = event.RetryableError;
        appendMessage(agent, makeItem("error", `第 ${retry.attempt}/${retry.max_retries} 次重试: ${retry.message}`));
      } else if (event.Error !== undefined) {
        appendMessage(agent, makeItem("error", event.Error));
      } else if (event.ContextList !== undefined) {
        // One agent, one context: the reply is the context itself (or
        // null when the agent has none yet).
        const next: ContextInfo | null = event.ContextList.context;
        setContexts((current) => {
          const copy = { ...current };
          if (next) copy[agent] = next;
          else delete copy[agent];
          return copy;
        });
        setActiveContexts((current) => ({ ...current, [agent]: next?.id ?? null }));
      } else if (event.ContextActivated !== undefined) {
        delete loadedHistory.current[agent];
        setActiveContexts((current) => ({ ...current, [agent]: event.ContextActivated.id }));
        setMessages((current) => ({ ...current, [agent]: [] }));
        streamingRef.current[agent] = null;
        setStreaming((current) => ({ ...current, [agent]: null }));
        setPlans((current) => ({ ...current, [agent]: event.PlanUpdate ?? current[agent] }));
      } else if (event.UsageUpdate !== undefined) {
        setStreaming((current) => {
          const item = current[agent];
          if (!item) return current;
          item.usage = formatTokenUsage({
            input: event.UsageUpdate.input_tokens ?? 0,
            output: event.UsageUpdate.output_tokens ?? 0,
            cache: event.UsageUpdate.cache_read_input_tokens ?? 0,
          });
        setUsage((currentUsage) => ({
          ...currentUsage,
          [agent]: {
            input: event.UsageUpdate.input_tokens ?? 0,
            output: event.UsageUpdate.output_tokens ?? 0,
            cache: event.UsageUpdate.cache_read_input_tokens ?? 0,
          },
        }));
          return { ...current, [agent]: { ...item } };
        });
      } else if (event.LifecycleChanged !== undefined) {
        const lifecycle = event.LifecycleChanged;
        const name = typeof lifecycle === "string" ? lifecycle : Object.keys(lifecycle)[0] ?? "idle";
        setStatuses((current) => ({ ...current, [agent]: name.toLowerCase() }));
        if (["idle", "cancelled", "error", "waiting", "aborted"].includes(name.toLowerCase())) {
          streamingRef.current[agent] = null;
          setStreaming((current) => ({ ...current, [agent]: null }));
        }
      }
      return;
    }

    if (frame.kind === "host") {
      const event = frame.data;
      if (event.kind === "agent_registered") {
        // Upsert: replaces the optimistic spawn stub (agent_id null, no
        // model) with the daemon's full info once registration lands.
        setAgents((current) => current.some((agent) => agent.path === event.info.path)
          ? current.map((agent) => agent.path === event.info.path ? event.info : agent)
          : [...current, event.info]);
      } else if (event.kind === "agent_model_changed") {
        setAgents((current) => current.map((agent) => agent.path === event.path
          ? { ...agent, model: event.spec, context_length: event.context_length }
          : agent));
      } else if (event.kind === "agent_unregistered") {
        // The live instance went away — under virtual-actor mode this is
        // usually idle-TTL passivation (event.cause "idle_passivation"),
        // not death. Drop it from the live registry only: open tabs
        // survive as dormant views and the identity re-activates from
        // storage on the next message.
        setAgents((current) => current.filter((agent) => agent.path !== event.path));
        setStatuses((current) => ({ ...current, [event.path]: "idle" }));
      } else if (event.kind === "identity_renamed") {
        // Cross-client rename: the storage record moved old_path →
        // new_path. Keep the picker list current; a live instance was
        // evicted (its unregistered frame precedes this) and re-activates
        // under the new path on demand.
        setStoredAgents((current) => current.map((record) =>
          record.id === event.agent_id ? { ...record, name: event.new_path } : record));
      } else if (event.kind === "identity_deleted") {
        // Cross-client delete: drop the record from the picker list. The
        // deleting client's own UI handles tab teardown.
        setStoredAgents((current) => current.filter((record) => record.id !== event.agent_id));
      } else if (event.kind === "agent_status_changed") {
        setAgents((current) => current.map((agent) => agent.path === event.path ? { ...agent, status: event.status } : agent));
        // The host projection collapses Retrying into Running; the retry
        // detail survives only in last_event — keep the UI on "retrying"
        // so an in-flight retry is not masked as a plain run. Its "failed"
        // projection is the unified enum's Error variant.
        const label0 = statusLabel(event.status);
        const label = label0 === "failed" ? "error" : label0;
        const lastEvent = typeof event.last_event === "string" ? event.last_event : "";
        const merged = label === "running" && /retry/i.test(lastEvent) ? "retrying" : label;
        setStatuses((current) => ({
          ...current,
          [event.path]: merged,
        }));
      }
    }
  }, [appendMessage]);

  const handleFrameRef = useRef(handleFrame);
  handleFrameRef.current = handleFrame;
  const recordFrameRef = useRef(recordFrame);
  recordFrameRef.current = recordFrame;

  /** Searches the persisted identity list for the agent picker — one
   *  bounded page (server-side `q` + `limit`, newest first), never the
   *  full table: under virtual-actor mode the record count is unbounded
   *  and loading it all stalled the UI. */
  const refreshStoredAgents = useCallback(async (query?: string) => {
    try {
      const needle = query?.trim() ?? "";
      const suffix = needle
        ? `?q=${encodeURIComponent(needle)}&limit=100`
        : "?limit=100";
      const page = await gatewayRequest("GET", `/storage/agents${suffix}`);
      // Guard the shape: a non-array reply (proxy error page parsed as
      // an object, a future envelope) must not crash every consumer of
      // `storedAgents.filter`.
      setStoredAgents(Array.isArray(page) ? page : []);
    } catch {
      // The picker degrades to live-only; a transient storage failure
      // should not surface as a modal error.
    }
  }, []);
  const hydrate = useCallback(async () => {
    try {
      const probe = await probeGateway();
      if (!probe.running) throw new Error(`${probe.addr} (未认证)`);
      const snapshot: StateSnapshot = await gatewayRequest("GET", "/state");
      // Tolerate a snapshot without the contexts map (mock gateways,
      // degraded backends) instead of failing the whole hydration.
      const snapshotContexts: Record<string, ContextInfo> = snapshot.contexts ?? {};
      const nextActiveContexts: Record<string, string | null> = {};
      for (const agent of Object.keys(snapshotContexts)) {
        nextActiveContexts[agent] = snapshotContexts[agent]?.id ?? null;
      }
      const nextAgents = snapshot.agents ?? [];
      // Reconnects re-run hydrate; keep the agent the user selected if it
      // is still an open tab instead of snapping back to the first one.
      const previous = stateRef.current.activeAgent;
      const livePaths = new Set(nextAgents.map((agent) => agent.path));
      // Tabs whose agent is no longer live stay open (dormant views);
      // a fresh session with no persisted tabs opens the first live
      // agent so the workbench never starts blank.
      const nextTabs = openTabsRef.current.length
        ? openTabsRef.current
        : nextAgents[0] ? [nextAgents[0].path] : [];
      const nextAgent = nextTabs.includes(previous)
        ? previous
        : nextTabs[0] ?? nextAgents[0]?.path ?? "";
      stateRef.current = {
        agents: nextAgents,
        contexts: snapshotContexts,
        activeAgent: nextAgent,
        activeContexts: nextActiveContexts,
        storedIds: stateRef.current.storedIds,
      };
      setAgents(nextAgents);
      setOpenTabs(nextTabs);
      setContexts(snapshotContexts);
      setActiveContexts(nextActiveContexts);
      setDisplay(snapshot.display_settings ?? {
        collapse_thinking: false,
        collapse_tool_calls: false,
        collapse_tool_results: false,
      });
      setActiveAgent(nextAgent);
      setStatuses(() => ({
        ...Object.fromEntries(nextAgents.map((agent) => [agent.path, statusLabel(agent.status)])),
        // Dormant tabs keep their dormant marker across reconnects.
        ...Object.fromEntries(nextTabs.filter((path) => !livePaths.has(path)).map((path) => [path, "idle"])),
      }));
      setConnected(true);
      setConnectionError(null);
      setBooting(false);
      void refreshStoredAgents();
      await startGatewayEvents(snapshot.last_seq, {
        onFrame: (frame) => {
          recordFrameRef.current(frame);
          handleFrameRef.current(frame);
        },
        onStatus: (status) => {
          setConnected(status !== "disconnected");
          if (status === "reconnected") void hydrateRef.current?.();
        },
      });
      const nextContext = nextActiveContexts[nextAgent];
      if (nextAgent && nextContext) void ensureAgentHistory(nextAgent, nextContext);
    } catch (cause) {
      setConnected(false);
      const unauthorized = cause instanceof GatewayHttpError && cause.status === 401;
      setNeedsLogin(unauthorized);
      setConnectionError(unauthorized ? "需要 Gateway 令牌" : String(cause));
      setBooting(false);
    }
  }, [ensureAgentHistory, refreshStoredAgents]);

  const hydrateRef = useRef(hydrate);
  hydrateRef.current = hydrate;
  const loadHistoryRef = useRef(loadHistory);
  loadHistoryRef.current = loadHistory;

  // Exchange the gateway token for a session cookie. The token is
  // posted once and dropped from JS state immediately afterwards — it
  // is never stored, logged, or put into the DOM.
  const submitLogin = async () => {
    const token = loginToken.trim();
    if (!token || loginBusy) return;
    setLoginBusy(true);
    setLoginError(null);
    try {
      await loginGateway(token);
      setLoginToken("");
      setNeedsLogin(false);
      await hydrateRef.current?.();
    } catch (cause) {
      setLoginError(String(cause));
    } finally {
      setLoginBusy(false);
    }
  };

  const logout = async () => {
    await logoutGateway();
    setNeedsLogin(true);
    setConnected(false);
  };

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  // Persist the open-view list; failures (private mode, quota) degrade
  // to session-only tabs.
  useEffect(() => {
    try { localStorage.setItem(OPEN_TABS_KEY, JSON.stringify(openTabs)); } catch { /* storage unavailable */ }
  }, [openTabs]);

  const sendMessageTo = async (path: string, context = "") => {
    const text = (draftsRef.current[path] ?? "").trim();
    if (!text || !path || sendingRef.current.has(path)) return;
    sendingRef.current.add(path);
    setSendingPaths(current => ({ ...current, [path]: true }));
    setDraftFor(path, "");
    try {
      await gatewayRequest("POST", `/agents/${path}/messages`, { text: text + context });
    } catch (cause) {
      setDrafts(current => ({ ...current, [path]: current[path] || text }));
      appendMessage(path, makeItem("error", String(cause)));
    } finally {
      sendingRef.current.delete(path);
      setSendingPaths(current => ({ ...current, [path]: false }));
    }
  };
  const sendMessage = (context = "") => sendMessageTo(activeAgent, context);

  /** Selecting a tab is a view operation: it both focuses the tab and
   *  opens it (spawn stubs and dormant identities included). */
  const selectAgent = (path: string) => {
    setOpenTabs((current) => current.includes(path) ? current : [...current, path]);
    setActiveAgent(path);
  };

  /** Closing a tab is a pure view operation: the live registry and the
   *  persisted identity are untouched. The daemon owns lifecycle. */
  const closeTab = (path: string) => {
    const index = openTabs.indexOf(path);
    const next = openTabs.filter((tab) => tab !== path);
    setOpenTabs(next);
    if (activeAgent === path) {
      const fallback = next[Math.max(0, index - 1)] ?? "";
      setActiveAgent(fallback);
      const contextId = activeContexts[fallback] ?? contexts[fallback]?.id;
      if (fallback && contextId) void ensureAgentHistory(fallback, contextId);
    }
  };

  /** Admin op: passivate a live instance now (fire-and-forget reply).
   *  Used by the delete-agent flow so a deleted identity leaves no live
   *  zombie behind. Also closes the tab — a deleted identity has no
   *  view to keep. */
  const shutdownAgent = useCallback(async (path: string) => {
    setBusy(`shutdown:${path}`);
    try {
      await gatewayRequest("POST", `/agents/${path}/shutdown`);
      setAgents((current) => current.filter((agent) => agent.path !== path));
      closeTab(path);
      void refreshStoredAgents();
    } catch (cause) {
      window.alert(String(cause));
    } finally {
      setBusy(null);
    }
  }, [closeTab, refreshStoredAgents]);

  const spawnAgent = async (input: { name: string; parentPath: string; profile: string }) => {
    setBusy("spawn");
    try {
      const result = await gatewayRequest("POST", "/agents", {
        name: input.name,
        parent_path: input.parentPath,
        profile: input.profile,
        model_spec: null,
      });
      if (result?.path) {
        setAgents((current) => current.some((agent) => agent.path === result.path) ? current : [
          ...current,
          { path: result.path, agent_id: null, status: "Idle" },
        ]);
        selectAgent(result.path);
      }
    } catch (cause) {
      window.alert(String(cause));
    } finally {
      setBusy(null);
    }
  };


  const resumeAgent = async (stored: StoredAgent) => {
    const running = agents.find((agent) => agent.agent_id === stored.id);
    if (running) {
      selectAgent(running.path);
      return;
    }
    selectAgent(stored.name);
  };

  /** Model catalog for the composer switcher; fetched once, on first open. */
  const loadModelCatalog = useCallback(async () => {
    if (modelCatalogLoaded.current) return;
    modelCatalogLoaded.current = true;
    try {
      const catalog = await gatewayRequest("GET", "/model-config");
      setModelCatalog(catalog?.models ?? []);
    } catch (cause) {
      modelCatalogLoaded.current = false;
      throw cause;
    }
  }, []);

  /** Hot-swaps an agent's model (`provider:model` spec); the daemon also
   *  persists the preference and broadcasts `agent_model_changed`, which
   *  keeps every workspace in sync — the local update only covers a
   *  missed event and is idempotent with the reducer. */
  const setAgentModel = async (path: string, spec: string) => {
    try {
      await gatewayRequest("PUT", `/agents/${path}/model`, { spec });
      setAgents((current) => current.map((agent) => agent.path === path
        ? { ...agent, model: spec }
        : agent));
      // The effective reasoning effort comes from the model, so re-read it.
      void refreshAgentEffort(path);
    } catch (cause) {
      appendMessage(path, makeItem("error", `切换模型失败: ${String(cause)}`));
    }
  };

  /** Effective reasoning effort per agent path (`undefined` = not loaded,
   *  `null` = the model has no thinking support). Read from the model
   *  info endpoint, which resolves override + model default. */
  const [agentEfforts, setAgentEfforts] = useState<Record<string, string | null>>({});
  const [agentModels, setAgentModels] = useState<Record<string, { model: string; context_length: number }>>({});
  const refreshAgentEffort = useCallback(async (path: string) => {
    if (!path) return;
    try {
      const info = await gatewayRequest("GET", `/agents/${path}/model`);
      setAgentEfforts((current) => ({ ...current, [path]: info?.reasoning_effort ?? null }));
      setAgentModels((current) => ({ ...current, [path]: { model: info.model, context_length: info.context_length } }));
    } catch { /* the composer pill falls back to the generic label */ }
  }, []);

  /** Sets the per-agent reasoning-effort override. The config endpoint
   *  replaces the whole runtime-override object, so read-modify-write;
   *  effort applies at request time, i.e. hot. */
  const setAgentEffort = async (path: string, effort: string) => {
    try {
      const config = await gatewayRequest("GET", `/agents/${path}/config`);
      const runtime = { ...(config?.runtime ?? {}), reasoning_effort: effort };
      await gatewayRequest("PUT", `/agents/${path}/config`, { runtime });
      setAgentEfforts((current) => ({ ...current, [path]: effort }));
    } catch (cause) {
      appendMessage(path, makeItem("error", `切换推理力度失败: ${String(cause)}`));
    }
  };

  const toggleDisplay = async (key: keyof DisplaySettings) => {
    const next = !display[key];
    setDisplay((current) => ({ ...current, [key]: next }));
    try {
      await gatewayRequest("PUT", "/settings", { key, value: next ? "1" : "0" });
    } catch (cause) {
      setDisplay((current) => ({ ...current, [key]: !next }));
      window.alert(String(cause));
    }
  };

  const activeContextId = activeContexts[activeAgent] ?? contexts[activeAgent]?.id ?? null;

  return {
    // connection & login
    booting,
    connected,
    connectionError,
    needsLogin,
    loginToken,
    setLoginToken,
    loginError,
    loginBusy,
    submitLogin,
    logout,
    // agent registry + view windows (tabs)
    agents,
    openTabs,
    activeAgent,
    statuses,
    selectAgent,
    spawnAgent,
    shutdownAgent,
    resumeAgent,
    storedAgents,
    refreshStoredAgents,
    closeTab,
    // contexts (the agent's sole working context)
    contexts,
    activeContexts,
    activeContextId,
    // transcript
    messages,
    compactions,
    historyLoading,
    streaming,
    rawMessages,
    promptHistories,
    plans,
    usage,
    display,
    toggleDisplay,
    refreshHistory: () => void loadHistory(activeAgent),
    ensureAgentHistory,
    // composer
    draft,
    setDraft,
    drafts,
    setDraftFor,
    busy: sendingPaths[activeAgent] ? "send" : busy,
    sendingPaths,
    sendMessage,
    sendMessageTo,
    // model switching
    modelCatalog,
    loadModelCatalog,
    setAgentModel,
    // reasoning effort
    agentEfforts,
    agentModels,
    refreshAgentEffort,
    setAgentEffort,
    // workbench panel feeds
    frameLog,
    notices,
  };
}
