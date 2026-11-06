/** 六门校验报告渲染：门列表 + 通过/失败状态 + 原因。 */

export interface GateResult {
  name: string;
  status: "pass" | "fail" | "blocked" | string;
  reason?: string | null;
}

export interface ValidationReport {
  report_id: string;
  plugin_name: string;
  overall?: string;
  gates: GateResult[];
}

export function ValidationReportView({ report }: { report: ValidationReport }) {
  return (
    <div role="group" aria-label="校验报告" data-validation-report className="grid gap-1 p-2 text-xs">
      {report.gates.map((gate) => (
        <div
          key={gate.name}
          data-gate={gate.name}
          data-gate-status={gate.status}
          className="flex items-start gap-2 rounded border px-2 py-1"
        >
          <span
            aria-hidden
            className={
              gate.status === "pass" ? "text-emerald-500" : gate.status === "fail" ? "text-destructive" : "text-muted-foreground"
            }
          >
            {gate.status === "pass" ? "✓" : gate.status === "fail" ? "✗" : "–"}
          </span>
          <span className="font-medium">{gate.name}</span>
          {gate.reason && <span className="min-w-0 flex-1 text-muted-foreground">{gate.reason}</span>}
        </div>
      ))}
    </div>
  );
}
