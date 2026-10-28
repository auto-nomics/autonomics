// App shell: top-level navigation, view switching, workspace hosting and the
// login/boot screens. All agent state and gateway wiring lives in
// use-agent-runtime; this component only renders it.
import { useCallback, useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { useAgentRuntime } from "@/app/use-agent-runtime";
import { AgentView, SpawnAgentDialog, AgentWorkspace, layoutSideOpen, readAgentLayout } from "@/features/agent";
import { BibliographyView } from "@/features/bibliography";
import { ConfigView, DagView, KmsView } from "@/features/system";
import { useMediaQuery } from "@/hooks/use-media-query";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog";

type View = "agent" | "config" | "bib" | "kms";

export function App() {
  const runtime = useAgentRuntime();
  const [view, setView] = useState<View>("agent");
  const [spawnOpen, setSpawnOpen] = useState(false);
  const [dagAgent, setDagAgent] = useState<string | null>(null);
  // Workspace hosting: the center column is a single AgentWorkspace
  // instance that portals into the agent view's slot and, once the user
  // switches to another view, into a floating panel instead.
  const [dockedSlot, setDockedSlot] = useState<HTMLDivElement | null>(null);
  // ≥ md: the inspector is a resizable panel; below it it is a drawer. The
  // flag starts from the persisted panel layout so a reload keeps it.
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const savedLayout = useRef(readAgentLayout()).current;
  const [rightOpen, setRightOpen] = useState(() => isDesktop && layoutSideOpen(savedLayout, "inspector"));

  // Viewport crossings reset the sidebar: drawer closed on mobile, panel
  // open on desktop. The first run is skipped so the restored layout — not
  // the default — stays in charge on initial load.
  const crossedOnceRef = useRef(false);
  useEffect(() => {
    if (!crossedOnceRef.current) {
      crossedOnceRef.current = true;
      return;
    }
    setRightOpen(isDesktop);
  }, [isDesktop]);

  // Stable callback ref for the workspace slot; the guard avoids the
  // detach/attach churn a fresh closure would cause on every render.
  const onSlotChange = useCallback((slot: HTMLDivElement | null) => {
    setDockedSlot((current) => (current === slot ? current : slot));
  }, []);

  if (runtime.booting) {
    return <div className="grid h-full place-items-center text-sm text-muted-foreground">正在连接 Gateway…</div>;
  }

  if (runtime.needsLogin) {
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
              void runtime.submitLogin();
            }}
          >
            <Input
              type="password"
              autoComplete="off"
              autoFocus
              placeholder="Gateway 令牌"
              value={runtime.loginToken}
              onChange={(event) => runtime.setLoginToken(event.target.value)}
            />
            <Button type="submit" disabled={runtime.loginBusy || !runtime.loginToken.trim()}>
              {runtime.loginBusy ? "连接中…" : "连接"}
            </Button>
          </form>
          {runtime.loginError ? (
            <pre className="mt-3 text-sm text-destructive">{runtime.loginError}</pre>
          ) : null}
        </Card>
      </div>
    );
  }

  if (!runtime.connected && !runtime.agents.length) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">Gateway 未运行</h1>
          <pre className="mt-3 text-sm text-destructive">{runtime.connectionError}</pre>
        </Card>
      </div>
    );
  }

  return (
    <main className="grid h-full min-h-0 grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)]">
      <header className="flex h-13 items-center justify-between gap-2 border-b bg-muted px-3">
        <nav className="flex min-w-0 items-center gap-1 overflow-x-auto whitespace-nowrap [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {(["agent", "config", "bib", "kms"] as View[]).map((item) => (
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
          <Badge variant={runtime.connected ? "default" : "destructive"}>
            <span className="hidden sm:inline">{runtime.connected ? "Gateway 已连接" : "已断开"}</span>
            <span className="sm:hidden">{runtime.connected ? "已连接" : "断开"}</span>
          </Badge>
          <Button
            size="sm"
            variant="ghost"
            title="结束浏览器会话"
            onClick={() => void runtime.logout()}
          >
            退出登录
          </Button>
        </div>
      </header>

      {view === "agent" ? (
        <AgentView
          activeAgent={runtime.activeAgent}
          activeSessionId={runtime.activeSessionId}
          agents={runtime.agents}
          sessions={runtime.sessions[runtime.activeAgent] ?? []}
          statuses={runtime.statuses}
          messages={runtime.messages[runtime.activeAgent] ?? []}
          plan={runtime.plans[runtime.activeAgent] ?? null}
          usage={runtime.usage[runtime.activeAgent] ?? null}
          display={runtime.display}
          promptHistory={runtime.promptHistories[runtime.activeAgent] ?? []}
          rawMessages={runtime.rawMessages[`${runtime.activeAgent}:${runtime.activeSessionId}`] ?? []}
          rightOpen={rightOpen}
          isDesktop={isDesktop}
          onSetRight={setRightOpen}
          onSlotChange={onSlotChange}
          onSpawn={() => setSpawnOpen(true)}
          onDraftChange={runtime.setDraft}
          onCancel={() => void gatewayRequest("POST", `/agents/${runtime.activeAgent}/cancel`).catch((cause) => window.alert(String(cause)))}
          onCompact={() => void gatewayRequest("POST", `/agents/${runtime.activeAgent}/compact`).catch((cause) => window.alert(String(cause)))}
          onShutdown={(path) => void runtime.shutdownAgent(path)}
          onSpawnAgent={runtime.spawnAgent}
          onResumeAgent={runtime.resumeAgent}
          onCreateSession={runtime.createSession}
          onRenameSession={runtime.renameSession}
          onCloseSession={runtime.closeSession}
          onActivateSession={(sessionId) => runtime.activateSession(runtime.activeAgent, sessionId)}
          onForkMessage={runtime.forkMessage}
          onToggleDisplay={(key) => void runtime.toggleDisplay(key)}
          onView={setView}
          onOpenDag={() => { if (runtime.activeAgent) setDagAgent(runtime.activeAgent); }}
          onRefreshHistory={runtime.refreshHistory}
        />
      ) : view === "bib" ? (
        <section className="flex min-h-0 flex-col p-3"><BibliographyView /></section>
      ) : (
        <section className="flex min-h-0 flex-col p-3">
          {view === "config" && <ConfigView activeAgent={runtime.activeAgent} />}
          {view === "kms" && <KmsView />}
        </section>
      )}

      <Dialog open={dagAgent !== null} onOpenChange={(open) => { if (!open) setDagAgent(null); }}>
        <DialogContent className="flex h-[min(80vh,800px)] w-[min(94vw,1100px)] max-w-none flex-col sm:max-w-none">
          <DialogHeader>
            <DialogTitle>Agent DAG</DialogTitle>
            <DialogDescription>{dagAgent}</DialogDescription>
          </DialogHeader>
          {dagAgent && <DagView key={dagAgent} activeAgent={dagAgent} />}
        </DialogContent>
      </Dialog>

      <SpawnAgentDialog
        open={spawnOpen}
        onOpenChange={setSpawnOpen}
        busy={runtime.busy}
        onSpawnAgent={runtime.spawnAgent}
      />

      {/* Rendered outside the view switch so its state survives; docks
          back into the agent view's slot when the view returns. */}
      <AgentWorkspace
        mode={view === "agent" ? (dockedSlot ? "docked" : "standby") : "floating"}
        dockedSlot={dockedSlot}
        agents={runtime.agents}
        activeAgent={runtime.activeAgent}
        statuses={runtime.statuses}
        messages={runtime.messages[runtime.activeAgent] ?? []}
        historyLoading={runtime.historyLoading[runtime.activeAgent] ?? false}
        streaming={runtime.streaming[runtime.activeAgent] ?? null}
        usage={runtime.usage[runtime.activeAgent] ?? null}
        display={runtime.display}
        draft={runtime.draft}
        busy={runtime.busy}
        rightOpen={rightOpen}
        onDraftChange={runtime.setDraft}
        onSendMessage={() => void runtime.sendMessage()}
        onSelectAgent={runtime.selectAgent}
        onShutdown={(path) => void runtime.shutdownAgent(path)}
        onSpawn={() => setSpawnOpen(true)}
        onToggleRight={() => setRightOpen((current) => !current)}
        onOpenDag={() => { if (runtime.activeAgent) setDagAgent(runtime.activeAgent); }}
      />
    </main>
  );
}
