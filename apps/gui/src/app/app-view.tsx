// App shell: boot/login gates, the context basket scope and the workbench
// shell. All agent state and gateway wiring lives in use-agent-runtime;
// the workbench (activity bar / side bar / editor area / panel / aux bar)
// lives in app/workbench/.
import { useTranslation } from "react-i18next";
import { ContextBasketProvider } from "@/features/context/context-basket";
import { useAgentRuntime } from "@/app/use-agent-runtime";
import { openAgentTab } from "@/app/workbench/static-commands";
import { WorkbenchShell } from "@/app/workbench/workbench-shell";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export function App() {
  const { t } = useTranslation();
  const runtime = useAgentRuntime();

  if (runtime.booting) {
    return <div className="grid h-full place-items-center text-sm text-muted-foreground">{t("app.boot.connecting")}</div>;
  }

  if (runtime.needsLogin) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">{t("app.boot.tokenTitle")}</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {t("app.boot.tokenBody")}
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
              placeholder={t("app.boot.tokenPlaceholder")}
              value={runtime.loginToken}
              onChange={(event) => runtime.setLoginToken(event.target.value)}
            />
            <Button type="submit" disabled={runtime.loginBusy || !runtime.loginToken.trim()}>
              {runtime.loginBusy ? t("app.boot.connectingBusy") : t("app.boot.connect")}
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
          <h1 className="text-xl font-medium">{t("app.boot.gatewayDown")}</h1>
          <pre className="mt-3 text-sm text-destructive">{runtime.connectionError}</pre>
        </Card>
      </div>
    );
  }

  return (
    <ContextBasketProvider
      scope={runtime.activeAgent && runtime.activeContextId ? JSON.stringify([runtime.activeAgent, runtime.activeContextId]) : null}
      contextLabel={runtime.activeAgent.split("/").pop() || t("app.boot.noAgent")}
      // Adding a context resource opens the active agent's editor tab so
      // the basket strip is visible where the chat lives.
      onAdd={() => openAgentTab(runtime.activeAgent)}
    >
      <WorkbenchShell runtime={runtime} />
    </ContextBasketProvider>
  );
}
