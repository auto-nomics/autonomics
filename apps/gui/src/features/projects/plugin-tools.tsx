import { useEffect, useState } from "react";
import { FileCheck, Play, Rocket } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { NodeTestRun, type ProjectNode } from "./node-test-run";
import { ValidationReportView, type ValidationReport } from "./validation-report";

/** Lifecycle tools share the same saved workspace as the code editor. */
export function PluginTools({ project, dirty, disabled, saveVersion, onChanged }: {
  project: string;
  dirty: boolean;
  disabled: boolean;
  saveVersion: number;
  onChanged: () => Promise<void>;
}) {
  const [dock, setDock] = useState<"report" | "test-run" | null>(null);
  const [nodes, setNodes] = useState<ProjectNode[]>([]);
  const [report, setReport] = useState<ValidationReport | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let cancelled = false;
    void gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/nodes`)
      .then(result => { if (!cancelled) setNodes(result.nodes ?? []); })
      .catch(() => { if (!cancelled) setNodes([]); });
    return () => { cancelled = true; };
  }, [project, saveVersion, dock]);

  const validate = async (activate: boolean) => {
    setBusy(true); setMessage(""); setDock("report"); setReport(null);
    try {
      const result = await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/${activate ? "activate" : "validate"}`);
      setReport(result.report ?? null);
      if (activate) setMessage(result.activated ? "已激活，节点已注册。" : "校验未通过，请查看报告。");
      await onChanged();
    } catch (cause) { setMessage(String(cause)); }
    finally { setBusy(false); }
  };
  return <div className="flex shrink-0 flex-col border-t">
    <div className="flex flex-wrap items-center gap-2 px-3 py-2">
      <Button size="sm" variant="outline" disabled={disabled || dirty || busy} onClick={() => void validate(false)}><FileCheck />校验</Button>
      <Button size="sm" variant="outline" disabled={disabled || dirty || busy} onClick={() => void validate(true)}><Rocket />激活</Button>
      <Button size="sm" variant="outline" data-dock-tab="test-run" disabled={disabled || dirty || busy} onClick={() => setDock("test-run")}><Play />试跑</Button>
      <Button size="sm" variant="ghost" data-dock-tab="report" onClick={() => setDock("report")}>报告</Button>
      {dirty && <span className="text-xs text-muted-foreground">请先保存修改再校验、激活或试跑。</span>}
      {dock && <Button size="sm" variant="ghost" className="ml-auto" onClick={() => setDock(null)}>收起开发面板</Button>}
    </div>
    {message && <div role="status" className="px-3 pb-2 text-xs">{message}</div>}
    <div hidden={!dock} className="max-h-72 overflow-y-auto border-t">
      {dock === "report" && (report ? <ValidationReportView report={report} /> : <p className="p-3 text-xs text-muted-foreground">{busy ? "正在校验…" : "还没有校验报告，点击「校验」生成。"}</p>)}
      {dock === "test-run" && (disabled || dirty ? <p className="p-3 text-xs text-muted-foreground">请连接开发容器并保存修改后试跑。</p> : <NodeTestRun project={project} nodes={nodes} />)}
    </div>
  </div>;
}
