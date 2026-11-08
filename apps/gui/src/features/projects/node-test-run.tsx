/**
 * Node test-run panel: pick a declared node, fill its file input ports
 * with sample text (params as free JSON validated server-side), run it
 * against the production container contract, and inspect the declared
 * outputs.
 */

import { useCallback, useEffect, useState } from "react";
import { Play } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/features/vfs";

interface NodePort { label: string | null; type: string }
interface NodeOutput { path: string; label: string | null; optional: boolean }
export interface ProjectNode {
  kind: string;
  desc: string;
  inputs: NodePort[];
  outputs: NodeOutput[];
}

interface TestRunOutput { path: string; present: boolean; optional: boolean; content?: string | null; truncated: boolean }
interface TestRunResult {
  exit_code: number;
  stdout: string;
  stderr: string;
  outputs: TestRunOutput[];
  workdir: string;
}

export function NodeTestRun({ project, nodes }: { project: string; nodes: ProjectNode[] }) {
  const [kind, setKind] = useState(nodes[0]?.kind ?? "");
  const [inputs, setInputs] = useState<Record<string, string>>({});
  const [params, setParams] = useState("{}");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<TestRunResult | null>(null);
  const selected = nodes.find((node) => node.kind === kind);

  useEffect(() => {
    setKind(nodes[0]?.kind ?? "");
    setInputs({});
    setResult(null);
    setError("");
  }, [project, nodes]);

  const run = useCallback(async () => {
    if (!kind || running) return;
    setRunning(true);
    setError("");
    setResult(null);
    try {
      let parsed: unknown = {};
      try {
        parsed = params.trim() ? JSON.parse(params) : {};
      } catch {
        throw new Error("参数不是合法 JSON");
      }
      const response = await gatewayRequest(
        "POST",
        `/plugin-projects/${encodeURIComponent(project)}/nodes/${encodeURIComponent(kind)}/test-run`,
        { kind, inputs, params: parsed },
      );
      setResult(response as TestRunResult);
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setRunning(false);
    }
  }, [project, kind, inputs, params, running]);

  if (nodes.length === 0) {
    return <p className="p-3 text-xs text-muted-foreground">manifest 里还没有节点声明。</p>;
  }

  return (
    <div className="grid min-h-0 flex-1 content-start gap-2 overflow-y-auto p-2 text-xs" data-test-run={project}>
      <label className="grid gap-1">
        节点
        <select
          aria-label="选择节点"
          className="h-8 rounded-md border bg-background px-2"
          value={kind}
          onChange={(event) => {
            setKind(event.target.value);
            setInputs({});
            setResult(null);
          }}
        >
          {nodes.map((node) => (
            <option key={node.kind} value={node.kind}>{node.kind} — {node.desc}</option>
          ))}
        </select>
      </label>

      {selected?.inputs.map((input) => (
        <label key={input.label ?? "?"} className="grid gap-1">
          入口 {input.label}
          {input.type === "file" ? (
            <textarea
              aria-label={`入口 ${input.label}`}
              data-test-input={input.label}
              rows={3}
              className="rounded-md border bg-background p-2 font-mono"
              value={inputs[input.label ?? ""] ?? ""}
              onChange={(event) => setInputs((current) => ({ ...current, [input.label ?? ""]: event.target.value }))}
            />
          ) : (
            <span className="text-muted-foreground">(tuple 端口暂不支持试跑)</span>
          )}
        </label>
      ))}

      <label className="grid gap-1">
        参数 (JSON)
        <textarea
          aria-label="节点参数 JSON"
          rows={2}
          className="rounded-md border bg-background p-2 font-mono"
          value={params}
          onChange={(event) => setParams(event.target.value)}
        />
      </label>

      <div>
        <Button size="sm" disabled={running || !kind} onClick={() => void run()}>
          <Play /> {running ? "运行中…" : "试跑"}
        </Button>
      </div>

      {error && <div role="alert" className="rounded border border-destructive/30 bg-destructive/10 p-2 text-destructive">{error}</div>}

      {result && (
        <div data-test-run-result className="grid gap-2 rounded border p-2">
          <div className="flex items-center gap-2">
            <span data-exit-code={result.exit_code} className={result.exit_code === 0 ? "text-emerald-500" : "text-destructive"}>
              退出码 {result.exit_code}
            </span>
            <span className="truncate text-muted-foreground" title={result.workdir}>workdir: {result.workdir}</span>
          </div>
          {result.stdout && (
            <details>
              <summary>stdout</summary>
              <pre className="max-h-40 overflow-auto rounded bg-muted/50 p-2 font-mono">{result.stdout}</pre>
            </details>
          )}
          {result.stderr && (
            <details open={result.exit_code !== 0}>
              <summary>stderr</summary>
              <pre className="max-h-40 overflow-auto rounded bg-muted/50 p-2 font-mono text-destructive">{result.stderr}</pre>
            </details>
          )}
          {result.outputs.map((output) => (
            <div key={output.path} data-test-output={output.path} className="grid gap-1 rounded border p-2">
              <div className="flex items-center gap-2">
                <span className="font-medium">出口 {output.path}</span>
                <span className={output.present ? "text-emerald-500" : output.optional ? "text-muted-foreground" : "text-destructive"}>
                  {output.present ? (output.truncated ? "已截断" : "已产出") : output.optional ? "可选未产出" : "缺失"}
                </span>
                {output.content && <span className="text-muted-foreground">{formatBytes(output.content.length)}</span>}
              </div>
              {output.content && (
                <pre className="max-h-40 overflow-auto rounded bg-muted/50 p-2 font-mono">{output.content}</pre>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
