import { useEffect, useState } from "react";
import { FileCheck, Play, Rocket } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation("projects");
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
      if (activate) setMessage(result.activated ? t("tools.activated") : t("tools.validateFailed"));
      await onChanged();
    } catch (cause) { setMessage(String(cause)); }
    finally { setBusy(false); }
  };
  return <div className="flex shrink-0 flex-col border-t">
    <div className="flex flex-wrap items-center gap-2 px-3 py-2">
      <Button size="sm" variant="outline" disabled={disabled || dirty || busy} onClick={() => void validate(false)}><FileCheck />{t("tools.validate")}</Button>
      <Button size="sm" variant="outline" disabled={disabled || dirty || busy} onClick={() => void validate(true)}><Rocket />{t("tools.activate")}</Button>
      <Button size="sm" variant="outline" data-dock-tab="test-run" disabled={disabled || dirty || busy} onClick={() => setDock("test-run")}><Play />{t("tools.testRun")}</Button>
      <Button size="sm" variant="ghost" data-dock-tab="report" onClick={() => setDock("report")}>{t("tools.report")}</Button>
      {dirty && <span className="text-xs text-muted-foreground">{t("tools.saveFirst")}</span>}
      {dock && <Button size="sm" variant="ghost" className="ml-auto" onClick={() => setDock(null)}>{t("tools.collapse")}</Button>}
    </div>
    {message && <div role="status" className="px-3 pb-2 text-xs">{message}</div>}
    <div hidden={!dock} className="max-h-72 overflow-y-auto border-t">
      {dock === "report" && (report ? <ValidationReportView report={report} /> : <p className="p-3 text-xs text-muted-foreground">{busy ? t("tools.validating") : t("tools.noReport")}</p>)}
      {dock === "test-run" && (disabled || dirty ? <p className="p-3 text-xs text-muted-foreground">{t("tools.connectFirst")}</p> : <NodeTestRun project={project} nodes={nodes} />)}
    </div>
  </div>;
}
