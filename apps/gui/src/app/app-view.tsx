// App shell: boot/login gates, the context basket scope and the workbench
// shell. All agent state and gateway wiring lives in use-agent-runtime;
// the workbench (activity bar / side bar / editor area / panel / aux bar)
// lives in app/workbench/.
import { ContextBasketProvider } from "@/features/context/context-basket";
import { useAgentRuntime } from "@/app/use-agent-runtime";
import { openAgentTab } from "@/app/workbench/static-commands";
import { WorkbenchShell } from "@/app/workbench/workbench-shell";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export function App() {
  const runtime = useAgentRuntime();

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
    <ContextBasketProvider
      scope={runtime.activeAgent && runtime.activeContextId ? JSON.stringify([runtime.activeAgent, runtime.activeContextId]) : null}
      contextLabel={runtime.activeAgent.split("/").pop() || "未选择 Agent"}
      // Adding a context resource opens the active agent's editor tab so
      // the basket strip is visible where the chat lives.
      onAdd={() => openAgentTab(runtime.activeAgent)}
    >
      <WorkbenchShell runtime={runtime} />
    </ContextBasketProvider>
  );
}
