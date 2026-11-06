import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { gatewayRequest } from "@/gateway";
import { useTheme } from "@/hooks/use-theme";
import { Button } from "@/components/ui/button";

export function ProjectTerminal({ project }: { project: string }) {
  const host = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const { resolved } = useTheme();
  const [error, setError] = useState("");
  const [exited, setExited] = useState(false);
  const [generation, setGeneration] = useState(0);
  const base = `/plugin-projects/${encodeURIComponent(project)}/terminal`;

  useEffect(() => {
    if (!host.current) return;
    let cancelled = false;
    let timer = 0;
    let offset = 0;
    let inputQueue = Promise.resolve();
    const terminal = new Terminal({ cursorBlink: true, fontFamily: "monospace", fontSize: 13, scrollback: 5000, convertEol: false });
    terminalRef.current = terminal;
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    const resize = () => {
      try {
        fit.fit();
        if (!cancelled) void gatewayRequest("PUT", `${base}/size`, { cols: terminal.cols, rows: terminal.rows }).catch(() => {});
      } catch { /* The panel can briefly have zero size while being resized. */ }
    };
    const observer = new ResizeObserver(resize);
    const element = host.current;
    const poll = async () => {
      try {
        const result = await gatewayRequest("GET", `${base}?offset=${offset}`);
        if (cancelled) return;
        if (result.truncated) terminal.write("\r\n[部分终端输出已过期]\r\n");
        if (result.data.length) terminal.write(new Uint8Array(result.data));
        offset = result.offset;
        if (result.exited) { terminal.write("\r\n[终端已退出]\r\n"); setExited(true); return; }
        timer = window.setTimeout(() => void poll(), 150);
      } catch (cause) { if (!cancelled) setError(String(cause)); }
    };
    void (async () => {
      try {
        setError("");
        setExited(false);
        await gatewayRequest("POST", base);
        if (cancelled) return;
        terminal.onData(data => {
          inputQueue = inputQueue.then(() => gatewayRequest("POST", `${base}/input`, { data })).catch(cause => { if (!cancelled) setError(String(cause)); });
        });
        observer.observe(element);
        resize();
        terminal.focus();
        void poll();
      } catch (cause) { if (!cancelled) setError(String(cause)); }
    })();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      observer.disconnect();
      terminal.dispose();
      terminalRef.current = null;
    };
  }, [base, generation]);

  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.theme = resolved === "dark" ? { background: "#10141b", foreground: "#e5e7eb" } : { background: "#ffffff", foreground: "#1f2937" };
  }, [resolved, project, generation]);

  return <div className="flex min-h-0 flex-1 flex-col">
    {(error || exited) && <div className="flex items-center gap-2 border-b px-3 py-1 text-xs"><span role={error ? "alert" : "status"} className={error ? "text-destructive" : "text-muted-foreground"}>{error ? `终端连接失败：${error}` : "终端已退出"}</span><Button size="xs" variant="outline" onClick={() => setGeneration(value => value + 1)}>重新连接</Button></div>}
    <div ref={host} aria-label={`${project} 容器终端`} className="min-h-0 flex-1 overflow-hidden p-2" />
  </div>;
}
