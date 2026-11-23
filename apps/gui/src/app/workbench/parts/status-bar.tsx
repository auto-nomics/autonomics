/**
 * Status bar: gateway connection, the active agent + lifecycle tone, the
 * active model and the context token usage. VSCode-dense (24px row).
 */

import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { AgentStatusIcon, leafName, statusLabel } from "@/features/agent";
import { TokenUsageBadge } from "@/features/agent/token-usage";
import { LanguageSwitcher } from "@/i18n/language-switcher";
import type { AgentRuntime } from "../runtime-context";
import { useActiveEditorContext } from "../active-editor-context";

export function StatusBar({ runtime }: { runtime: AgentRuntime }) {
  const { t } = useTranslation();
  const { editor, kind } = useActiveEditorContext();
  const activeInfo = runtime.agents.find((agent) => agent.path === runtime.activeAgent);
  const usage = runtime.usage[runtime.activeAgent] ?? null;
  const running = statusLabel(runtime.statuses[runtime.activeAgent]);

  return (
    <footer className="flex h-6 shrink-0 items-center gap-3 border-t bg-sidebar px-2 text-[11px] text-sidebar-foreground" aria-label={t("workbench.statusbar.label")}>
      <span
        className="flex shrink-0 items-center gap-1.5"
        title={runtime.connected ? t("workbench.statusbar.connected") : t("workbench.statusbar.disconnected")}
      >
        <span aria-hidden className={cn("size-2 rounded-full", runtime.connected ? "bg-primary" : "bg-destructive")} />
        {runtime.connected ? t("workbench.statusbar.connected") : t("workbench.statusbar.disconnected")}
      </span>
      {runtime.activeAgent && (
        <span className="flex min-w-0 items-center gap-1.5" title={runtime.activeAgent}>
          <AgentStatusIcon status={runtime.statuses[runtime.activeAgent]} />
          <span className="truncate">{leafName(runtime.activeAgent)}</span>
          <span className="shrink-0 text-muted-foreground">{running}</span>
        </span>
      )}
      <span className="hidden min-w-0 items-center gap-1 truncate text-muted-foreground sm:flex" title={t("workbench.statusbar.model")}>
        {activeInfo?.model ?? t("workbench.statusbar.noModel")}
      </span>
      {editor && <span aria-label={t("workbench.statusbar.editor")} title={kind?.name?.(editor) ?? editor.id} className="min-w-0 truncate text-muted-foreground">{kind?.title(editor) ?? editor.kind}</span>}
      {usage ? <TokenUsageBadge usage={usage} className="ml-auto shrink-0 text-[10px]" /> : null}
      <LanguageSwitcher className="ml-auto size-4 shrink-0 [&_svg]:size-3" />
    </footer>
  );
}
