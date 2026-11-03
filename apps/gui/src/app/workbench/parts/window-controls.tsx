import { useEffect, useState } from "react";
import { Maximize2, Minus, X } from "lucide-react";
import { isTauri } from "@/lib/is-tauri";
import { Button } from "@/components/ui/button";

export function WindowControls({ vertical = false }: { vertical?: boolean }) {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const window = getCurrentWindow();
        setMaximized(await window.isMaximized());
        const promise = window.onResized(async () => setMaximized(await window.isMaximized()));
        unlisten = () => void promise.then((fn) => fn());
      } catch {
        /* non-Tauri build: controls are not rendered anyway */
      }
    })();
    return () => unlisten?.();
  }, []);

  const withWindow = (action: (window: import("@tauri-apps/api/window").Window) => void) => () => {
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        action(getCurrentWindow());
      } catch { /* not in Tauri */ }
    })();
  };

  return (
    <div className={vertical ? "flex flex-col items-center" : "flex items-center"}>
      <Button size="icon-sm" variant="ghost" aria-label="最小化" title="最小化" onClick={withWindow((w) => w.minimize())}><Minus /></Button>
      <Button size="icon-sm" variant="ghost" aria-label={maximized ? "还原" : "最大化"} title={maximized ? "还原" : "最大化"} onClick={withWindow((w) => w.toggleMaximize())}>
        {maximized ? <MinimizeIcon /> : <Maximize2 />}
      </Button>
      <Button size="icon-sm" variant="ghost" aria-label="关闭窗口" title="关闭" className="hover:bg-destructive hover:text-white" onClick={withWindow((w) => w.close())}><X /></Button>
    </div>
  );
}

function MinimizeIcon() {
  // Restore (unmaximize): two overlapping rectangles.
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="size-4" aria-hidden>
      <path d="M8 8h12v12H8z" />
      <path d="M4 16V4h12" />
    </svg>
  );
}

