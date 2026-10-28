import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import "./app.css";
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
  SessionInfo,
  StateSnapshot,
  StoredAgent,
} from "@/types";
import {
  AgentView,
  SpawnAgentDialog,
  type DisplaySettings,
} from "@/features/agent";
import { layoutSideOpen, readAgentLayout } from "@/features/agent-layout";
import { AgentWorkspace } from "@/features/agent-workspace";
import { useMediaQuery } from "@/hooks/use-media-query";
import { BibliographyView } from "@/features/bibliography";
import { ConfigView } from "@/features/config";
import { KmsView } from "@/features/kms";
import { DagView } from "@/features/dag";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

type View = "agent" | "config" | "bib" | "kms" | "dag";
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
  const normalized = typeof status === "string"
    ? status
    : status && typeof status === "object" && "kind" in status
      ? String((status as { kind: unknown }).kind)
      : "idle";
  const value = normalized.toLowerCase();
  if (value.includes("stream") || value.includes("requesting") || value.includes("executing")) return value;
  return value.replace(/^.*\./, "");
}

export default function App() {
  const [booting, setBooting] = useState(true);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  // The web build authenticates with a session cookie; when the daemon
  // rejects the request the user is asked for the gateway token once.
  const [needsLogin, setNeedsLogin] = useState(false);
  const [loginToken, setLoginToken] = useState("");
  const [loginError, setLoginError] = useState<string | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [view, setView] = useState<View>("agent");
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Record<string, SessionInfo[]>>({});
  const [activeSessions, setActiveSessions] = useState<Record<string, string | null>>({});
  const [activeAgent, setActiveAgent] = useState("");
  const [statuses, setStatuses] = useState<Record<string, string>>({});
  const [messages, setMessages] = useState<MessageMap>({});
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
  // Workspace hosting: the center column is a single AgentWorkspace
  // instance that portals into the agent view's slot and, once the user
  // switches to another view, into a floating panel instead.
  const [dockedSlot, setDockedSlot] = useState<HTMLDivElement | null>(null);
  const [spawnOpen, setSpawnOpen] = useState(false);
  // ≥ md: sidebars are resizable panels; below it they are drawers. The
  // flags start from the persisted panel layout so a reload keeps both.
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const savedLayout = useMemo(() => readAgentLayout(), []);
  const [leftOpen, setLeftOpen] = useState(() => isDesktop && layoutSideOpen(savedLayout, "agents"));
  const [rightOpen, setRightOpen] = useState(() => isDesktop && layoutSideOpen(savedLayout, "inspector"));

  // Viewport crossings reset the sidebars: drawers closed on mobile, panels
  // open on desktop. The first run is skipped so the restored layout — not
  // the default — stays in charge on initial load.
  const crossedOnceRef = useRef(false);
  useEffect(() => {
    if (!crossedOnceRef.current) {
      crossedOnceRef.current = true;
      return;
    }
    setLeftOpen(isDesktop);
    setRightOpen(isDesktop);
  }, [isDesktop]);

  // Stable callback ref for the workspace slot; the guard avoids the
  // detach/attach churn a fresh closure would cause on every render.
  const onSlotChange = useCallback((slot: HTMLDivElement | null) => {
    setDockedSlot((current) => (current === slot ? current : slot));
  }, []);

  const stateRef = useRef({ agents, sessions, activeAgent, activeSessions });
  stateRef.current = { agents, sessions, activeAgent, activeSessions };
  const streamingRef = useRef<Record<string, ChatItem | null>>({});

  const appendMessage = useCallback((agent: string, item: ChatItem) => {
    setMessages((current) => ({ ...current, [agent]: [...(current[agent] ?? []), item] }));
  }, []);

  const loadHistory = useCallback(async (agentPath: string) => {
    const info = stateRef.current.agents.find((agent) => agent.path === agentPath);
    const sessionId = stateRef.current.activeSessions[agentPath]
      ?? latestSession(stateRef.current.sessions[agentPath] ?? []);
    if (!info?.agent_id || !sessionId) return;
    try {
      const response = await gatewayRequest(
        "GET",
        `/agents/${info.agent_id}/sessions/${sessionId}/history`,
      );
      const history: ChatItem[] = (Array.isArray(response) ? response : response.messages ?? [])
        .flatMap(convertMessage);
      setMessages((current) => ({ ...current, [agentPath]: history }));
      setRawMessages((current) => ({
        ...current,
        [`${agentPath}:${sessionId}`]: Array.isArray(response) ? response : response.messages ?? [],
      }));
    } catch (cause) {
      appendMessage(agentPath, makeItem("error", `加载历史失败: ${String(cause)}`));
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
      } else if (event.SessionClosed !== undefined) {
        setSessions((current) => {
          const next = (current[agent] ?? []).filter((session) => session.id !== event.SessionClosed.id);
          setActiveSessions((currentActive) => {
            if (currentActive[agent] !== event.SessionClosed.id) return currentActive;
            const fallback = latestSession(next);
            if (fallback) void loadHistoryRef.current(agent);
            return { ...currentActive, [agent]: fallback };
          });
          return { ...current, [agent]: next };
        });
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
        setAgents((current) => current.some((agent) => agent.path === event.info.path) ? current : [...current, event.info]);
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
        setStatuses((current) => ({ ...current, [event.path]: statusLabel(event.status) }));
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

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  const sendMessage = async () => {
    const text = draft.trim();
    if (!text || !activeAgent || busy === "send") return;
    setBusy("send");
    setDraft("");
    try {
      await gatewayRequest("POST", `/agents/${activeAgent}/messages`, { text });
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

  const deleteAgent = async (path: string, agentId: string | null) => {
    if (!agentId || !window.confirm(`Permanently delete ${path}? Stored sessions cannot be recovered.`)) return;
    setBusy(`delete:${path}`);
    try {
      await gatewayRequest("DELETE", `/storage/agents/${encodeURIComponent(agentId)}`);
      await gatewayRequest("POST", `/agents/${path}/shutdown`);
      forgetAgent(path);
    } catch (cause) {
      window.alert(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const createSession = async (title: string) => {
    if (!activeAgent) return;
    await gatewayRequest("POST", `/agents/${activeAgent}/sessions`, {
      title: title.trim() || "New session",
      fork_from: null,
      fork_message_id: null,
    });
  };

  const renameSession = async (sessionId: string, title: string) => {
    await gatewayRequest("PATCH", `/agents/${activeAgent}/sessions/${sessionId}/title`, {
      title: title.trim() || "Untitled",
    });
    setSessions((current) => ({
      ...current,
      [activeAgent]: (current[activeAgent] ?? []).map((session) => session.id === sessionId ? { ...session, title } : session),
    }));
  };

  const closeSession = async (sessionId: string) => {
    await gatewayRequest("POST", `/agents/${activeAgent}/sessions/${sessionId}/close`);
    setSessions((current) => ({
      ...current,
      [activeAgent]: (current[activeAgent] ?? []).filter((session) => session.id !== sessionId),
    }));
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

  const forkMessage = async (message: { id: string; text: string }) => {
    const sessionId = activeSessions[activeAgent];
    if (!sessionId) throw new Error("Fork requires an active session");
    await gatewayRequest("POST", `/agents/${activeAgent}/sessions`, {
      title: `Fork at ${message.text.split(/\r?\n/)[0].slice(0, 80)}`,
      fork_from: sessionId,
      fork_message_id: message.id,
    });
    await gatewayRequest("GET", `/agents/${activeAgent}/sessions`);
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

  const activateSession = async (agentPath: string, sessionId: string) => {
    setActiveSessions((current) => ({ ...current, [agentPath]: sessionId }));
    setMessages((current) => ({ ...current, [agentPath]: [] }));
    try {
      await gatewayRequest("POST", `/agents/${agentPath}/sessions/${sessionId}/activate`);
      if (agentPath === activeAgent) void loadHistory(agentPath);
    } catch (cause) {
      window.alert(String(cause));
    }
  };

  if (booting) {
    return <div className="grid h-full place-items-center text-sm text-muted-foreground">正在连接 Gateway…</div>;
  }

  if (needsLogin) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">需要 Gateway 令牌</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            令牌在运行 Gateway 的机器上:~/.autonomics/gateway.token
            (或环境变量 AUTONOMICS_HTTP_API_TOKEN)。令牌只提交一次,
            之后由 HttpOnly 会话 Cookie 维持登录。
          </p>
          <form
            className="mt-4 flex items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void submitLogin();
            }}
          >
            <Input
              type="password"
              autoComplete="off"
              autoFocus
              placeholder="Gateway 令牌"
              value={loginToken}
              onChange={(event) => setLoginToken(event.target.value)}
            />
            <Button type="submit" disabled={loginBusy || !loginToken.trim()}>
              {loginBusy ? "连接中…" : "连接"}
            </Button>
          </form>
          {loginError ? (
            <pre className="mt-3 text-sm text-destructive">{loginError}</pre>
          ) : null}
        </Card>
      </div>
    );
  }

  if (!connected && !agents.length) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">Gateway 未运行</h1>
          <pre className="mt-3 text-sm text-destructive">{connectionError}</pre>
        </Card>
      </div>
    );
  }

  const activeSessionId = activeSessions[activeAgent] ?? latestSession(sessions[activeAgent] ?? []);

  return (
    <main className="grid h-full min-h-0 grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)]">
      <header className="flex h-13 items-center justify-between gap-2 border-b bg-muted px-3">
        <nav className="flex min-w-0 items-center gap-1 overflow-x-auto whitespace-nowrap [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {(["agent", "config", "bib", "kms", "dag"] as View[]).map((item) => (
            <Button
              key={item}
              size="sm"
              variant={view === item ? "secondary" : "ghost"}
              onClick={() => setView(item)}
            >
              {item === "agent" ? "Agent" : item === "bib" ? "Bibliography" : item.toUpperCase()}
            </Button>
          ))}
        </nav>
        <div className="flex shrink-0 items-center gap-2">
          <Badge variant={connected ? "default" : "destructive"}>
            <span className="hidden sm:inline">{connected ? "Gateway 已连接" : "已断开"}</span>
            <span className="sm:hidden">{connected ? "已连接" : "断开"}</span>
          </Badge>
          <Button
            size="sm"
            variant="ghost"
            title="结束浏览器会话"
            onClick={() => {
              void logoutGateway().then(() => {
                setNeedsLogin(true);
                setConnected(false);
              });
            }}
          >
            退出登录
          </Button>
        </div>
      </header>

      {view === "agent" ? (
        <AgentView
          activeAgent={activeAgent}
          activeSessionId={activeSessionId}
          agents={agents}
          sessions={sessions[activeAgent] ?? []}
          statuses={statuses}
          messages={messages[activeAgent] ?? []}
          plan={plans[activeAgent] ?? null}
          usage={usage[activeAgent] ?? null}
          display={display}
          promptHistory={promptHistories[activeAgent] ?? []}
          rawMessages={rawMessages[`${activeAgent}:${activeSessionId}`] ?? []}
          leftOpen={leftOpen}
          rightOpen={rightOpen}
          isDesktop={isDesktop}
          onSetLeft={setLeftOpen}
          onSetRight={setRightOpen}
          onSlotChange={onSlotChange}
          onSpawn={() => setSpawnOpen(true)}
          onDraftChange={setDraft}
          onSelectAgent={selectAgent}
          onCancel={() => void gatewayRequest("POST", `/agents/${activeAgent}/cancel`).catch((cause) => window.alert(String(cause)))}
          onCompact={() => void gatewayRequest("POST", `/agents/${activeAgent}/compact`).catch((cause) => window.alert(String(cause)))}
          onShutdown={(path) => void shutdownAgent(path)}
          onDelete={(path, agentId) => void deleteAgent(path, agentId)}
          onSpawnAgent={spawnAgent}
          onResumeAgent={resumeAgent}
          onCreateSession={createSession}
          onRenameSession={renameSession}
          onCloseSession={closeSession}
          onActivateSession={(sessionId) => activateSession(activeAgent, sessionId)}
          onForkMessage={forkMessage}
          onToggleDisplay={(key) => void toggleDisplay(key)}
          onView={setView}
          onRefreshHistory={() => void loadHistory(activeAgent)}
        />
      ) : view === "bib" ? (
        <section className="flex min-h-0 flex-col p-3"><BibliographyView /></section>
      ) : (
        <section className="flex min-h-0 flex-col p-3">
          {view === "config" && <ConfigView activeAgent={activeAgent} />}
          {view === "kms" && <KmsView />}
          {view === "dag" && <DagView activeAgent={activeAgent} />}
        </section>
      )}

      <SpawnAgentDialog
        open={spawnOpen}
        onOpenChange={setSpawnOpen}
        busy={busy}
        onSpawnAgent={spawnAgent}
      />

      {/* Rendered outside the view switch so its state survives; docks
          back into the agent view's slot when the view returns. */}
      <AgentWorkspace
        mode={view === "agent" ? (dockedSlot ? "docked" : "standby") : "floating"}
        dockedSlot={dockedSlot}
        agents={agents}
        activeAgent={activeAgent}
        statuses={statuses}
        messages={messages[activeAgent] ?? []}
        streaming={streaming[activeAgent] ?? null}
        usage={usage[activeAgent] ?? null}
        display={display}
        draft={draft}
        busy={busy}
        leftOpen={leftOpen}
        rightOpen={rightOpen}
        onDraftChange={setDraft}
        onSendMessage={() => void sendMessage()}
        onSelectAgent={selectAgent}
        onShutdown={(path) => void shutdownAgent(path)}
        onSpawn={() => setSpawnOpen(true)}
        onToggleLeft={() => setLeftOpen((current) => !current)}
        onToggleRight={() => setRightOpen((current) => !current)}
      />
    </main>
  );
}

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
document.documentElement.classList.add("dark");
ReactDOM.createRoot(container).render(<App />);
