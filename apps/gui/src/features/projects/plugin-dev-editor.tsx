/**
 * `plugin-dev` editor body: the devcontainer-style dev window for one
 * plugin project — file tree + editor on top, terminal + action dock at
 * the bottom (校验 / 激活 / 试跑 / 停止 / 重启容器). Opening the window
 * starts the dev session on demand; the container is daemon-side and
 * survives the tab closing.
 */

import { useCallback, useEffect, useState } from "react";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { FileCode2, FlaskConical, RefreshCw, Rocket, SquareTerminal, StopCircle } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { EditorRef } from "@/app/workbench/workbench-store";
import { ProjectFileEditor } from "./project-file-editor";
import { DevTerminal } from "./dev-terminal";
import { NodeTestRun, type ProjectNode } from "./node-test-run";
import { ValidationReportView, type ValidationReport } from "./validation-report";

interface DevSessionView {
  plugin: string;
  state: "absent" | "created" | "running" | "exited" | "unknown" | string;
  container_name: string;
  image?: string | null;
  detail?: string | null;
}

type Dock = "terminal" | "test-run" | "report";

export function PluginDevEditorBody({ editor }: { editor: EditorRef }) {
  const project = String(editor.params.name ?? "");
  const [files, setFiles] = useState<string[]>([]);
  const [file, setFile] = useState("");
  const [nodes, setNodes] = useState<ProjectNode[]>([]);
  const [session, setSession] = useState<DevSessionView | null>(null);
  const [dock, setDock] = useState<Dock>("terminal");
  const [report, setReport] = useState<ValidationReport | null>(null);
  const [statusLine, setStatusLine] = useState("");
  const [busy, setBusy] = useState(false);

  const refreshFiles = useCallback(async () => {
    try {
      const result = await gatewayRequest(
        "GET",
        `/plugin-projects/${encodeURIComponent(project)}/files`,
      );
      setFiles(result.files ?? []);
      setFile((current) => (current && result.files?.includes(current) ? current : result.files?.[0] ?? ""));
    } catch (cause: unknown) {
      setStatusLine(String((cause as { message?: string } | null)?.message ?? cause));
    }
  }, [project]);

  const refreshNodes = useCallback(async () => {
    try {
      const result = await gatewayRequest(
        "GET",
        `/plugin-projects/${encodeURIComponent(project)}/nodes`,
      );
      setNodes(result.nodes ?? []);
    } catch { /* manifest mid-edit; the manager shows parse errors */ }
  }, [project]);

  const refreshSession = useCallback(async () => {
    try {
      const view: DevSessionView = await gatewayRequest(
        "GET",
        `/plugin-projects/${encodeURIComponent(project)}/dev-session`,
      );
      setSession(view);
      return view;
    } catch (cause: unknown) {
      setStatusLine(String((cause as { message?: string } | null)?.message ?? cause));
      return null;
    }
  }, [project]);

  useEffect(() => {
    void (async () => {
      await Promise.all([refreshFiles(), refreshNodes()]);
      const view = await refreshSession();
      // Devcontainer attach semantics: start the container when the
      // window opens and none is running.
      if (view && (view.state === "absent" || view.state === "exited" || view.state === "created")) {
        try {
          const started: DevSessionView = await gatewayRequest(
            "POST",
            `/plugin-projects/${encodeURIComponent(project)}/dev-session`,
            { action: "start" },
          );
          setSession(started);
        } catch (cause: unknown) {
          setStatusLine(String((cause as { message?: string } | null)?.message ?? cause));
        }
      }
    })();
  }, [project, refreshFiles, refreshNodes, refreshSession]);

  const sessionAction = async (action: "stop" | "restart") => {
    setBusy(true);
    try {
      const view: DevSessionView = await gatewayRequest(
        "POST",
        `/plugin-projects/${encodeURIComponent(project)}/dev-session`,
        { action },
      );
      setSession(view);
      setStatusLine("");
    } catch (cause: unknown) {
      setStatusLine(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const validate = async (activate: boolean) => {
    setBusy(true);
    setDock("report");
    try {
      const response = await gatewayRequest(
        "POST",
        `/plugin-projects/${encodeURIComponent(project)}/${activate ? "activate" : "validate"}`,
      );
      setReport(response.report ?? null);
      if (activate) {
        setStatusLine(
          response.activated
            ? "已激活：本地快照已安装并热注册，节点已进入 DAG。"
            : "激活被校验拦下（见报告）。",
        );
        await refreshSession();
      }
    } catch (cause: unknown) {
      setStatusLine(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const sessionTone =
    session?.state === "running" ? "text-emerald-500"
      : session?.state === "unknown" ? "text-amber-500"
        : "text-muted-foreground";

  return (
    <section aria-label={`插件开发 ${project}`} className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin-dev={project}>
      <header className="flex flex-wrap items-center gap-2 border-b px-3 py-2 text-sm">
        <FlaskConical className="size-4 shrink-0 text-primary" />
        <h1 className="font-semibold">{project}</h1>
        <span className={`text-xs ${sessionTone}`} data-dev-session-state={session?.state ?? "…"} title={session?.detail ?? session?.container_name ?? ""}>
          容器 {session?.state ?? "…"}
        </span>
        {session?.image && <span className="hidden max-w-48 truncate text-[10px] text-muted-foreground md:inline" title={session.image}>{session.image}</span>}
        <span className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void validate(false)}>校验</Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void validate(true)}><Rocket /> 激活</Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void sessionAction("restart")}><RefreshCw /> 重启容器</Button>
          <Button size="sm" variant="outline" disabled={busy || session?.state !== "running"} onClick={() => void sessionAction("stop")}><StopCircle /> 停止</Button>
        </span>
      </header>
      {statusLine && <div role="status" className="border-b bg-muted/40 px-3 py-1 text-xs">{statusLine}</div>}

      <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
        <ResizablePanel defaultSize={62} minSize={20}>
          <ResizablePanelGroup orientation="horizontal">
            <ResizablePanel defaultSize={26} minSize={15}>
              <aside className="flex h-full min-h-0 flex-col overflow-y-auto border-r bg-muted/20 p-1" aria-label="项目文件">
                <div className="px-2 py-1 text-[10px] font-semibold uppercase text-muted-foreground">文件</div>
                {files.map((item) => (
                  <button
                    type="button"
                    key={item}
                    data-project-file={item}
                    className={`flex items-center gap-1.5 rounded px-2 py-1 text-left text-xs hover:bg-accent ${item === file ? "bg-accent" : ""}`}
                    onClick={() => setFile(item)}
                  >
                    <FileCode2 className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="break-all">{item}</span>
                  </button>
                ))}
                {files.length === 0 && <p className="p-2 text-xs text-muted-foreground">（无文件）</p>}
              </aside>
            </ResizablePanel>
            <ResizableHandle className="w-1" />
            <ResizablePanel minSize={25}>
              {file ? (
                <ProjectFileEditor project={project} file={file} onSaved={() => void refreshNodes()} />
              ) : (
                <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">从左侧选择文件</div>
              )}
            </ResizablePanel>
          </ResizablePanelGroup>
        </ResizablePanel>
        <ResizableHandle className="h-1 w-auto" />
        <ResizablePanel defaultSize={38} minSize={15}>
          <div className="flex h-full min-h-0 flex-col">
            <div role="tablist" aria-label="开发面板" className="flex h-8 shrink-0 items-center gap-1 border-b px-2">
              {([
                ["terminal", "终端", SquareTerminal],
                ["test-run", "试跑", Play],
                ["report", "报告", FileCode2],
              ] as const).map(([id, label, Icon]) => (
                <button
                  key={id}
                  role="tab"
                  type="button"
                  aria-selected={dock === id}
                  data-dock-tab={id}
                  className={`flex items-center gap-1 rounded px-2 py-1 text-xs ${dock === id ? "bg-accent font-medium" : "text-muted-foreground hover:bg-accent"}`}
                  onClick={() => setDock(id)}
                >
                  <Icon className="size-3.5" /> {label}
                </button>
              ))}
            </div>
            {dock === "terminal" && <DevTerminal plugin={project} />}
            {dock === "test-run" && <NodeTestRun project={project} nodes={nodes} />}
            {dock === "report" && (
              report
                ? <div className="min-h-0 flex-1 overflow-y-auto"><ValidationReportView report={report} /></div>
                : <p className="p-3 text-xs text-muted-foreground">还没有校验报告；点「校验」生成。</p>
            )}
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>
    </section>
  );
}

function Play(props: { className?: string }) {
  return <Rocket className={props.className} />;
}
