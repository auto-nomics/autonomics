/**
 * xterm.js terminal over the dev-session WebSocket. Binary frames are
 * the pty stream; `{"type":"exit"}` shows a reconnect button (no
 * auto-respawn). Tauri desktop builds show a hint until the Rust side
 * grows a WS bridge (the web build is the supported surface).
 */

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { isTauri } from "@/lib/is-tauri";

type Phase = "connecting" | "open" | "closed";

export function DevTerminal({ plugin }: { plugin: string }) {
  const holder = useRef<HTMLDivElement | null>(null);
  const terminal = useRef<{ write: (data: string) => void; writeln: (line: string) => void; dispose: () => void; open: (el: HTMLElement) => void; reset: () => void } | null>(null);
  const socket = useRef<WebSocket | null>(null);
  const [phase, setPhase] = useState<Phase>("connecting");
  const [exitCode, setExitCode] = useState<number | null>(null);

  const connect = () => {
    setPhase("connecting");
    setExitCode(null);
    if (isTauri) {
      setPhase("closed");
      return;
    }
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${protocol}//${window.location.host}/api/v1/plugin-projects/${encodeURIComponent(plugin)}/terminal`;
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    socket.current = ws;

    ws.onopen = () => {
      setPhase("open");
      void (async () => {
        // xterm is a heavy dependency: load it lazily so the workbench
        // bundle stays lean.
        const [{ Terminal }, { FitAddon }] = await Promise.all([
          import("@xterm/xterm"),
          import("@xterm/addon-fit"),
        ]);
        import("@xterm/xterm/css/xterm.css");
        terminal.current?.dispose();
        const term = new Terminal({ fontSize: 12, cursorBlink: true, scrollback: 2000 });
        const fit = new FitAddon();
        term.loadAddon(fit);
        if (holder.current) {
          term.open(holder.current);
          fit.fit();
        }
        term.onData((data) => {
          if (ws.readyState === WebSocket.OPEN) ws.send(new TextEncoder().encode(data));
        });
        term.onResize(({ cols, rows }) => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: "resize", cols, rows }));
          }
        });
        terminal.current = term;
      })();
    };
    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        try {
          const control = JSON.parse(event.data);
          if (control.type === "exit") setExitCode(control.code ?? 0);
          if (control.type === "notice") terminal.current?.writeln(`\x1b[33m${control.message}\x1b[0m`);
        } catch { /* ignore */ }
        return;
      }
      const bytes = new Uint8Array(event.data);
      terminal.current?.write(new TextDecoder().decode(bytes));
    };
    ws.onclose = () => setPhase("closed");
    ws.onerror = () => setPhase("closed");
  };

  useEffect(() => {
    connect();
    return () => {
      socket.current?.close();
      terminal.current?.dispose();
      terminal.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [plugin]);

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-dev-terminal={plugin}>
      <div className="flex h-8 shrink-0 items-center gap-2 border-b px-2 text-xs">
        <span className="text-muted-foreground">容器终端</span>
        <span
          data-terminal-phase={phase}
          className={
            phase === "open" ? "text-emerald-500" : phase === "connecting" ? "text-amber-500" : "text-muted-foreground"
          }
        >
          {phase === "open" ? "已连接" : phase === "connecting" ? "连接中…" : exitCode !== null ? `已退出 (${exitCode})` : "未连接"}
        </span>
        {(phase === "closed" || exitCode !== null) && (
          <Button size="sm" variant="outline" onClick={connect}>重开 shell</Button>
        )}
      </div>
      <div ref={holder} className="min-h-0 flex-1 overflow-hidden bg-black p-1" aria-label={`${plugin} 容器终端`} />
    </div>
  );
}
