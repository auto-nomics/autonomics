import { useCallback, useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import "./app.css";
import {
  gatewayRequest,
  probeGateway,
  startGatewayEvents,
} from "@/gateway";
import type { AgentInfo, ChatItem, GatewayFrame, SessionInfo, StateSnapshot } from "@/types";
import { BibliographyView } from "@/features/bibliography";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

type View = "agent" | "config" | "bib" | "kms" | "dag";
type MessageMap = Record<string, ChatItem[]>;

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
    else if (block.type === "tool_result") output.push(makeItem("tool", `${block.is_error ? "ERR" : "OK"} ${block.content ?? ""}`, "tool_result"));
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

function Placeholder({ title }: { title: string }) {
  return (
    <Card className="grid flex-1 place-items-center p-8">
      <div className="text-center">
        <h2 className="text-lg font-medium">{title}</h2>
        <p className="mt-1 text-sm text-muted-foreground">React migration in progress for this view.</p>
      </div>
    </Card>
  );
}

export default function App() {
  const [booting, setBooting] = useState(true);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [view, setView] = useState<View>("agent");
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Record<string, SessionInfo[]>>({});
  const [activeSessions, setActiveSessions] = useState<Record<string, string | null>>({});
  const [activeAgent, setActiveAgent] = useState("");
  const [statuses, setStatuses] = useState<Record<string, string>>({});
  const [messages, setMessages] = useState<MessageMap>({});
  const [streaming, setStreaming] = useState<Record<string, ChatItem | null>>({});
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  const stateRef = useRef({ agents, sessions, activeAgent });
  stateRef.current = { agents, sessions, activeAgent };
  const streamingRef = useRef<Record<string, ChatItem | null>>({});

  const appendMessage = useCallback((agent: string, item: ChatItem) => {
    setMessages((current) => ({ ...current, [agent]: [...(current[agent] ?? []), item] }));
  }, []);

  const loadHistory = useCallback(async (agentPath: string) => {
    const info = stateRef.current.agents.find((agent) => agent.path === agentPath);
    const sessionId = latestSession(
      stateRef.current.sessions[agentPath] ?? [],
      stateRef.current.sessions[agentPath]?.length ? undefined : undefined,
    );
    if (!info?.agent_id || !sessionId) return;
    try {
      const response = await gatewayRequest(
        "GET",
        `/agents/${info.agent_id}/sessions/${sessionId}/history`,
      );
      const history: ChatItem[] = (Array.isArray(response) ? response : response.messages ?? [])
        .flatMap(convertMessage);
      setMessages((current) => ({ ...current, [agentPath]: history }));
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
      } else if (event.UserMessageAcknowledged !== undefined) {
        appendMessage(agent, makeItem("user", event.UserMessageAcknowledged));
      } else if (event.Error !== undefined) {
        appendMessage(agent, makeItem("error", event.Error));
      } else if (event.SessionList !== undefined) {
        const nextSessions: SessionInfo[] = event.SessionList.sessions;
        setSessions((current) => ({ ...current, [agent]: nextSessions }));
        setActiveSessions((current) => ({ ...current, [agent]: latestSession(nextSessions, current[agent]) }));
      } else if (event.SessionActivated !== undefined) {
        setActiveSessions((current) => ({ ...current, [agent]: event.SessionActivated.id }));
        setMessages((current) => ({ ...current, [agent]: [] }));
      } else if (event.UsageUpdate !== undefined) {
        setStreaming((current) => {
          const item = current[agent];
          if (!item) return current;
          item.usage = `输入 ${event.UsageUpdate.input_tokens ?? 0} · 输出 ${event.UsageUpdate.output_tokens ?? 0}`;
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
      if (!probe.running) throw new Error(`${probe.addr} (token ${probe.token_found ? "found" : "missing"})`);
      const snapshot: StateSnapshot = await gatewayRequest("GET", "/state");
      const nextActiveSessions: Record<string, string | null> = {};
      for (const [agent, agentSessions] of Object.entries(snapshot.sessions)) {
        nextActiveSessions[agent] = latestSession(agentSessions);
      }
      const nextAgents = snapshot.agents;
      const nextAgent = nextAgents[0]?.path ?? "";
      stateRef.current = { agents: nextAgents, sessions: snapshot.sessions, activeAgent: nextAgent };
      setAgents(nextAgents);
      setSessions(snapshot.sessions);
      setActiveSessions(nextActiveSessions);
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
      setConnectionError(String(cause));
      setBooting(false);
    }
  }, [loadHistory]);

  const hydrateRef = useRef(hydrate);
  hydrateRef.current = hydrate;

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  const sendMessage = async () => {
    const text = draft.trim();
    if (!text || !activeAgent || busy === "send") return;
    setBusy("send");
    setDraft("");
    appendMessage(activeAgent, makeItem("user", text));
    try {
      await gatewayRequest("POST", `/agents/${activeAgent}/messages`, { text });
    } catch (cause) {
      appendMessage(activeAgent, makeItem("error", String(cause)));
    } finally {
      setBusy(null);
    }
  };

  const spawnAgent = async () => {
    const name = window.prompt("New agent name");
    if (!name) return;
    setBusy("spawn");
    try {
      await gatewayRequest("POST", "/agents", { name, parent_path: "/root", profile: "researcher", model_spec: null });
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
    } catch (cause) {
      window.alert(String(cause));
    } finally {
      setBusy(null);
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

  const activeInfo = agents.find((agent) => agent.path === activeAgent);
  const activeMessages = messages[activeAgent] ?? [];
  const streamingItem = streaming[activeAgent];
  const activeSessionList = sessions[activeAgent] ?? [];

  return (
    <main className="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)]">
      <header className="flex h-13 items-center justify-between border-b bg-muted px-4">
        <nav className="flex items-center gap-1">
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
        <Badge variant={connected ? "default" : "destructive"}>
          {connected ? "Gateway 已连接" : "已断开"}
        </Badge>
      </header>

      {view === "agent" ? (
        <section className="grid min-h-0 grid-cols-[300px_minmax(0,1fr)]">
          <aside className="min-h-0 overflow-auto border-r bg-muted p-3">
            <div className="flex items-center justify-between">
              <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Agents</h2>
              <Button size="xs" onClick={() => void spawnAgent()} disabled={busy === "spawn"}>New</Button>
            </div>
            <div className="mt-3 space-y-1.5">
              {agents.map((agent) => (
                <div key={agent.path} className="rounded-lg border bg-card p-1.5">
                  <button
                    type="button"
                    onClick={() => {
                      setActiveAgent(agent.path);
                      void loadHistory(agent.path);
                    }}
                    className={cn("w-full rounded-md px-2 py-1.5 text-left", activeAgent === agent.path && "bg-accent")}
                  >
                    <span className="block truncate text-sm">{agent.path}</span>
                    <span className="text-xs text-muted-foreground">{statuses[agent.path] ?? statusLabel(agent.status)}</span>
                  </button>
                  <Button size="xs" variant="ghost" className="mt-1 w-full" onClick={() => void shutdownAgent(agent.path)}>
                    Shutdown
                  </Button>
                </div>
              ))}
            </div>
            <Separator className="my-4" />
            <h2 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Sessions</h2>
            <div className="mt-3 space-y-1.5">
              {activeSessionList.map((session) => (
                <button
                  key={session.id}
                  type="button"
                  onClick={() => void activateSession(activeAgent, session.id)}
                  className={cn(
                    "w-full rounded-lg border p-2 text-left",
                    activeSessions[activeAgent] === session.id && "border-primary bg-accent",
                  )}
                >
                  <span className="line-clamp-2 block text-sm">{session.title || session.id}</span>
                  <span className="text-xs text-muted-foreground">{new Date(session.last_active).toLocaleString()}</span>
                </button>
              ))}
            </div>
          </aside>

          <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)_auto]">
            <div className="flex items-center justify-between gap-3 border-b px-4 py-2.5">
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">{activeAgent || "No agent"}</div>
                <div className="truncate text-xs text-muted-foreground">
                  {activeInfo?.model ?? "no model"} · {statuses[activeAgent] ?? "idle"}
                </div>
              </div>
              <Button size="sm" variant="secondary" onClick={() => void gatewayRequest("POST", `/agents/${activeAgent}/compact`)}>
                Compact
              </Button>
            </div>
            <div className="min-h-0 space-y-2 overflow-auto p-4">
              {activeMessages.map((message) => (
                <article
                  key={message.id}
                  className={cn(
                    "rounded-lg border p-3 text-sm",
                    message.role === "user" && "border-primary/40 bg-card",
                    message.role === "assistant" && "bg-card",
                    message.role === "thinking" && "opacity-80",
                    message.role === "tool" && "font-mono text-xs",
                    message.role === "error" && "border-destructive/40 bg-destructive/10 text-destructive",
                  )}
                >
                  <div className="mb-1 text-xs text-muted-foreground uppercase">{message.role}</div>
                  <pre className="font-sans whitespace-pre-wrap">{message.text}</pre>
                  {message.usage && <div className="mt-1 text-xs text-muted-foreground">{message.usage}</div>}
                </article>
              ))}
              {streamingItem && !activeMessages.some((message) => message.id === streamingItem.id) && (
                <pre className="rounded-lg border bg-card p-3 text-sm whitespace-pre-wrap">{streamingItem.text}</pre>
              )}
            </div>
            <form
              className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 border-t bg-muted p-3"
              onSubmit={(event) => {
                event.preventDefault();
                void sendMessage();
              }}
            >
              <Textarea value={draft} rows={3} onChange={(event) => setDraft(event.target.value)} placeholder="输入消息" />
              <Button type="submit" disabled={!draft.trim() || busy === "send"}>发送</Button>
            </form>
          </div>
        </section>
      ) : view === "bib" ? (
        <section className="flex min-h-0 flex-col p-3"><BibliographyView /></section>
      ) : (
        <section className="flex min-h-0 flex-col p-3">
          {view === "config" && <Placeholder title="Model Config" />}
          {view === "kms" && <Placeholder title="KMS" />}
          {view === "dag" && <Placeholder title="DAG" />}
        </section>
      )}
    </main>
  );
}

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
document.documentElement.classList.add("dark");
ReactDOM.createRoot(container).render(<App />);
