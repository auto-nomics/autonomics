import { useSyncExternalStore } from "react";
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
  const context = useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => current);
  const build = context?.build;
  return <div className="min-h-0 flex-1 overflow-auto p-3 text-xs">
    {!context ? <p className="manuscript-muted">打开论文项目以查看编译日志。</p> : !build ? <p className="manuscript-muted">点击“编译”生成 PDF 并查看诊断。</p> : <>
      <p className="manuscript-panel-intro">{context.title} · 构建 {build.id} · {build.status}</p>
      {build.diagnostics?.map((diagnostic, index) => <button className="manuscript-diagnostic" key={index} onClick={() => context.navigate(diagnostic.file, diagnostic.line)}><strong>{diagnostic.severity}</strong> {diagnostic.message}{diagnostic.file && <small>{diagnostic.file}:{diagnostic.line}</small>}</button>)}
      <pre className="manuscript-log">{build.log || "没有编译输出。"}</pre>
    </>}
  </div>;
}
registerWorkbenchView({ id: "manuscript-log", title: "编译日志", icon: FileText, defaultLocation: "panel", body: ManuscriptLog });
