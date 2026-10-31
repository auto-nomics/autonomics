// Agent runtime store: gateway connection, login, the per-agent event
// reducer and every agent/session action. The shell (app-view) consumes this
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
  SessionInfo,
  StateSnapshot,
  StoredAgent,
} from "@/types";
import type { DisplaySettings } from "@/features/agent";

type MessageMap = Record<string, ChatItem[]>;
type RawMessageMap = Record<string, BackendMessage[]>;
type PromptHistoryMap = Record<string, string[]>;
type PlanMap = Record<string, { revision: number; update: any }>;
type UsageMap = Record<string, { input: number; output: number; cache: number }>;

let itemSequence = 0;
function makeItem(role: ChatItem["role"], text: string, kind?: ChatItem["kind"]): ChatItem {
  itemSequence += 1;
  return { id: itemSequence, role, text, kind, time: Date.now() };
}

function latestSession(sessions: SessionInfo[], activeId?: string | null) {
  if (activeId && sessions.some((session) => session.id === activeId)) return activeId;
  return [...sessions].sort((left, right) => right.last_active - left.last_active)[0]?.id ?? null;
}

function convertMessage(message: any): ChatItem[] {
  const output: ChatItem[] = [];
  let text = "";
  const pushText = () => {
    if (text) output.push(makeItem(message.role === "user" ? "user" : "assistant", text));
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
  if (status && typeof status === "object" && "kind" in status) {
    // Coarse host projection (runtime AgentStatus). The lifecycle Waiting
    // state collapses into AwaitingTool { tool: "wait_task" } — recover
    // the distinction from the payload so the UI shows "waiting" instead
    // of a generic tool indicator (or idle, pre-icon-rework).
    const { kind, tool } = status as { kind?: unknown; tool?: unknown };
    const tag = String(kind ?? "idle").toLowerCase();
    if (tag === "awaiting_tool") return tool === "wait_task" ? "waiting" : "toolrunning";
    if (tag === "failed") return "error";
    return tag;
  }
  const value = (typeof status === "string" ? status : "idle").toLowerCase();
  if (value.includes("stream") || value.includes("requesting") || value.includes("executing")) return value;
  return value.replace(/^.*\./, "");
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
  const [sessions, setSessions] = useState<Record<string, SessionInfo[]>>({});
  const [activeSessions, setActiveSessions] = useState<Record<string, string | null>>({});
  const [activeAgent, setActiveAgent] = useState("");
  const [statuses, setStatuses] = useState<Record<string, string>>({});
  const [messages, setMessages] = useState<MessageMap>({});
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
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  // Model catalog for the composer switcher; fetched lazily on first open.
  const [modelCatalog, setModelCatalog] = useState<ModelRow[]>([]);
  const modelCatalogLoaded = useRef(false);

  const stateRef = useRef({ agents, sessions, activeAgent, activeSessions });
  stateRef.current = { agents, sessions, activeAgent, activeSessions };
  const streamingRef = useRef<Record<string, ChatItem | null>>({});
  const historyLoadSeq = useRef<Record<string, number>>({});

  const appendMessage = useCallback((agent: string, item: ChatItem) => {
    setMessages((current) => ({ ...current, [agent]: [...(current[agent] ?? []), item] }));
  }, []);

  const loadHistory = useCallback(async (agentPath: string, explicitSessionId?: string) => {
    const info = stateRef.current.agents.find((agent) => agent.path === agentPath);
    const sessionId = explicitSessionId ?? stateRef.current.activeSessions[agentPath]
      ?? latestSession(stateRef.current.sessions[agentPath] ?? []);
    if (!info?.agent_id || !sessionId) return;
    // Only the latest in-flight load per agent may clear the flag, so a
    // superseded fetch (fast session double-switch) cannot hide the
    // skeleton while its successor is still running.
    const seq = (historyLoadSeq.current[agentPath] ?? 0) + 1;
    historyLoadSeq.current[agentPath] = seq;
    setHistoryLoading((current) => ({ ...current, [agentPath]: true }));
    try {
      const response = await gatewayRequest(
        "GET",
        `/agents/${info.agent_id}/sessions/${sessionId}/history`,
      );
      const history: ChatItem[] = (Array.isArray(response) ? response : response.messages ?? [])
        .flatMap(convertMessage);
      if (historyLoadSeq.current[agentPath] !== seq) return;
      setMessages((current) => ({ ...current, [agentPath]: history }));
      setRawMessages((current) => ({
        ...current,
        [`${agentPath}:${sessionId}`]: Array.isArray(response) ? response : response.messages ?? [],
      }));
    } catch (cause) {
      appendMessage(agentPath, makeItem("error", `加载历史失败: ${String(cause)}`));
    } finally {
      if (historyLoadSeq.current[agentPath] === seq) {
        setHistoryLoading((current) => ({ ...current, [agentPath]: false }));
      }
    }
  }, [appendMessage]);

  const handleFrame = useCallback((frame: GatewayFrame) => {
    if (frame.kind === "lag") {
      void hydrateRef.current?.();
      return;
    }
    if (frame.kind === "agent") {
      const agent = frame.data.agent;
      const event = frame.data.event;
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
      if (event.TextDelta !== undefined) {
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
        setPromptHistories((current) => {
          const history = current[agent] ?? [];
          return history.includes(event.UserMessageAcknowledged) ? current : { ...current, [agent]: [...history, event.UserMessageAcknowledged].slice(-200) };
        });
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
      } else if (event.SessionList !== undefined) {
        const nextSessions: SessionInfo[] = event.SessionList.sessions;
        setSessions((current) => ({ ...current, [agent]: nextSessions }));
        setActiveSessions((current) => ({ ...current, [agent]: latestSession(nextSessions, current[agent]) }));
      } else if (event.SessionActivated !== undefined) {
        setActiveSessions((current) => ({ ...current, [agent]: event.SessionActivated.id }));
        setMessages((current) => ({ ...current, [agent]: [] }));
        streamingRef.current[agent] = null;
        setStreaming((current) => ({ ...current, [agent]: null }));
        setPlans((current) => ({ ...current, [agent]: event.PlanUpdate ?? current[agent] }));
      } else if (event.UsageUpdate !== undefined) {
        setStreaming((current) => {
          const item = current[agent];
          if (!item) return current;
          item.usage = `输入 ${event.UsageUpdate.input_tokens ?? 0} · 输出 ${event.UsageUpdate.output_tokens ?? 0}`;
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
        setAgents((current) => current.filter((agent) => agent.path !== event.path));
        const snapshot = stateRef.current;
        if (snapshot.activeAgent === event.path) {
          const index = snapshot.agents.findIndex((agent) => agent.path === event.path);
          const rest = snapshot.agents.filter((agent) => agent.path !== event.path);
          const fallback = rest[Math.max(0, index - 1)]?.path ?? "";
          setActiveAgent(fallback);
          if (fallback) void loadHistoryRef.current(fallback);
        }
      } else if (event.kind === "agent_status_changed") {
        setAgents((current) => current.map((agent) => agent.path === event.path ? { ...agent, status: event.status } : agent));
        // The coarse host projection collapses Retrying into Running;
        // recover the distinction from last_event ("retrying (2/5): …")
        // so the spinner survives the back-off window.
        setStatuses((current) => ({
          ...current,
          [event.path]: typeof event.last_event === "string" && event.last_event.startsWith("retrying")
            ? "retrying"
            : statusLabel(event.status),
        }));
      }
    }
  }, [appendMessage]);

  const handleFrameRef = useRef(handleFrame);
  handleFrameRef.current = handleFrame;

  const hydrate = useCallback(async () => {
    try {
      const probe = await probeGateway();
      if (!probe.running) throw new Error(`${probe.addr} (未认证)`);
      const snapshot: StateSnapshot = await gatewayRequest("GET", "/state");
      const nextActiveSessions: Record<string, string | null> = {};
      for (const [agent, agentSessions] of Object.entries(snapshot.sessions)) {
        nextActiveSessions[agent] = latestSession(agentSessions);
      }
      const nextAgents = snapshot.agents;
      // Reconnects re-run hydrate; keep the agent the user selected if it
      // still exists instead of snapping back to the first one.
      const previous = stateRef.current.activeAgent;
      const nextAgent = nextAgents.some((agent) => agent.path === previous)
        ? previous
        : nextAgents[0]?.path ?? "";
      stateRef.current = {
        agents: nextAgents,
        sessions: snapshot.sessions,
        activeAgent: nextAgent,
        activeSessions: nextActiveSessions,
      };
      setAgents(nextAgents);
      setSessions(snapshot.sessions);
      setActiveSessions(nextActiveSessions);
      setDisplay(snapshot.display_settings);
      setActiveAgent(nextAgent);
      setStatuses(Object.fromEntries(nextAgents.map((agent) => [agent.path, statusLabel(agent.status)])));
      setConnected(true);
      setConnectionError(null);
      setBooting(false);
      await startGatewayEvents(snapshot.last_seq, {
        onFrame: (frame) => handleFrameRef.current(frame),
        onStatus: (status) => {
          setConnected(status !== "disconnected");
          if (status === "reconnected") void hydrateRef.current?.();
        },
      });
      if (nextAgent) void loadHistory(nextAgent);
    } catch (cause) {
      setConnected(false);
      const unauthorized = cause instanceof GatewayHttpError && cause.status === 401;
      setNeedsLogin(unauthorized);
      setConnectionError(unauthorized ? "需要 Gateway 令牌" : String(cause));
      setBooting(false);
    }
  }, [loadHistory]);

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

  const sendMessage = async (context = "") => {
    const text = draft.trim();
    if (!text || !activeAgent || busy === "send") return;
    setBusy("send");
    setDraft("");
    try {
      await gatewayRequest("POST", `/agents/${activeAgent}/messages`, { text: text + context });
      // The transcript entry and prompt-history entry both arrive via the
      // UserMessageAcknowledged event; echoing them here rendered the
      // message twice.
    } catch (cause) {
      setDraft(text);
      appendMessage(activeAgent, makeItem("error", String(cause)));
    } finally {
      setBusy(null);
    }
  };

  const selectAgent = (path: string) => {
    setActiveAgent(path);
    void loadHistory(path);
  };

  const forgetAgent = (path: string) => {
    const index = agents.findIndex((agent) => agent.path === path);
    const next = agents.filter((agent) => agent.path !== path);
    setAgents(next);
    if (activeAgent === path) {
      const fallback = next[Math.max(0, index - 1)]?.path ?? "";
      setActiveAgent(fallback);
      if (fallback) void loadHistory(fallback);
    }
  };

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

  const shutdownAgent = async (path: string) => {
    setBusy(`shutdown:${path}`);
    try {
      await gatewayRequest("POST", `/agents/${path}/shutdown`);
      forgetAgent(path);
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
    const name = stored.name.split("/").filter(Boolean).pop() ?? "";
    if (!name) throw new Error("Cannot resume the root placeholder");
    const segments = stored.name.split("/").filter(Boolean);
    const result = await gatewayRequest("POST", "/agents", {
      name,
      parent_path: segments.length <= 1 ? "/root" : `/${segments.slice(0, -1).join("/")}`,
      profile: stored.config_json.kind ?? "researcher",
      runtime: stored.config_json.runtime ?? {},
      model_spec: stored.config_json.preferred_model ?? null,
    });
    setAgents((current) => current.some((agent) => agent.path === result.path) ? current : [
      ...current,
      { path: result.path, agent_id: stored.id, status: "Idle" },
    ]);
    selectAgent(result.path);
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
  const refreshAgentEffort = useCallback(async (path: string) => {
    if (!path) return;
    try {
      const info = await gatewayRequest("GET", `/agents/${path}/model`);
      setAgentEfforts((current) => ({ ...current, [path]: info?.reasoning_effort ?? null }));
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

  const activeSessionId = activeSessions[activeAgent] ?? latestSession(sessions[activeAgent] ?? []);

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
    // agent registry
    agents,
    activeAgent,
    statuses,
    selectAgent,
    spawnAgent,
    shutdownAgent,
    resumeAgent,
    // sessions (the agent's sole working context)
    sessions,
    activeSessions,
    activeSessionId,
    // transcript
    messages,
    historyLoading,
    streaming,
    rawMessages,
    promptHistories,
    plans,
    usage,
    display,
    toggleDisplay,
    refreshHistory: () => void loadHistory(activeAgent),
    // composer
    draft,
    setDraft,
    busy,
    sendMessage,
    // model switching
    modelCatalog,
    loadModelCatalog,
    setAgentModel,
    // reasoning effort
    agentEfforts,
    refreshAgentEffort,
    setAgentEffort,
  };
}
