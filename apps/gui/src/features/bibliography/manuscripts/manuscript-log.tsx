import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import i18next from "i18next";
import { FileText } from "lucide-react";
import { registerWorkbenchView } from "@/app/workbench/view-registry";
import { moveWorkbenchView } from "@/app/workbench/view-layout";
import { setPanelVisible } from "@/app/workbench/workbench-store";
type LogContext = { title: string; build: { id: string; status: string; log: string; diagnostics: Array<{ severity: string; message: string; file?: string; line?: number }> } | null; navigate: (file?: string, line?: number) => void };
let current: LogContext | null = null;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach(listener => listener());
export function publishManuscriptLog(context: LogContext) {
  current = context; notify();
  return () => { if (current === context) { current = null; notify(); } };
}
export function openManuscriptLog() { moveWorkbenchView("manuscript-log", "panel"); setPanelVisible(true); }
function ManuscriptLog() {
  const { t } = useTranslation("bibliography");
  const context = useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => current);
  const build = context?.build;
  return <div className="min-h-0 flex-1 overflow-auto p-3 text-xs">
    {!context ? <p className="manuscript-muted">{t("manuscripts.log.empty")}</p> : !build ? <p className="manuscript-muted">{t("manuscripts.log.noBuild")}</p> : <>
      <p className="manuscript-panel-intro">{context.title} · {t("manuscripts.log.buildLabel", { id: build.id })} · {build.status}</p>
      {build.diagnostics?.map((diagnostic, index) => <button className="manuscript-diagnostic" key={index} onClick={() => context.navigate(diagnostic.file, diagnostic.line)}><strong>{diagnostic.severity}</strong> {diagnostic.message}{diagnostic.file && <small>{diagnostic.file}:{diagnostic.line}</small>}</button>)}
      <pre className="manuscript-log">{build.log || t("manuscripts.log.noOutput")}</pre>
    </>}
  </div>;
}
registerWorkbenchView({ id: "manuscript-log", title: i18next.t("bibliography:manuscripts.log.title"), icon: FileText, defaultLocation: "panel", body: ManuscriptLog });
